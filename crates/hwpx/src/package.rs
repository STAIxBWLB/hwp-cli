//! ZIP 패키지 수준 접근.
//!
//! `hwp info`/`hwp dump`가 OWPML 파싱 없이도 동작하도록
//! 컨테이너 계층만으로 메타데이터를 제공한다.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use quick_xml::events::Event;
use serde::Serialize;

use crate::error::{HwpxError, Result};
use crate::read::password;

pub const MIMETYPE: &str = "application/hwp+zip";

/// hwp-cli와 외부 오케스트레이터가 공유하는 단일 기본 ZIP 제한 프로파일 이름.
pub const NATIVE_PACKAGE_LIMITS_PROFILE_NAME: &str = "hwp-cli-native-v1";

/// 기본 제한값을 선택한 이유. [`NATIVE_PACKAGE_LIMITS_PROFILE`]과 함께 직렬화되어
/// Maru 같은 외부 오케스트레이터가 정책 이름·값·근거를 한 번에 동기화할 수 있다.
pub const NATIVE_PACKAGE_LIMITS_RATIONALE: &str = "일반적인 이미지 중심 HWPX는 허용하되 \
ZIP bomb, 과도한 중앙 디렉터리 이름 메모리, 비정상적으로 큰 XML을 입력 파싱 전에 제한";

/// 신뢰할 수 없는 HWPX 패키지를 처리할 때 적용하는 자원 제한.
///
/// ZIP 중앙 디렉터리의 선언값을 먼저 검사하고, 실제 압축 해제도 같은 한도로 다시
/// 감싼다. [`serde::Serialize`]을 구현하므로 기본 프로파일을 JSON 같은 기계 판독
/// 형식으로 그대로 노출할 수 있다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PackageLimits {
    /// 패키지 안의 최대 엔트리 수(디렉터리 엔트리 포함).
    pub max_entries: usize,
    /// 중앙 디렉터리 엔트리 이름 하나의 최대 원시 바이트 수.
    ///
    /// ZIP의 `u16` 이름 길이 전체를 수용한다. 실질적인 메모리 상한은 아래 합계
    /// 제한이 담당하며, 이 값도 이름 버퍼 할당 전에 검사한다.
    pub max_entry_name_bytes: u64,
    /// 중앙 디렉터리 전체 엔트리 이름의 최대 원시 바이트 합계.
    pub max_total_name_bytes: u64,
    /// 같은 이름의 엔트리를 거부할지 여부.
    pub reject_duplicate_names: bool,
    /// 엔트리 하나의 최대 압축 해제 크기.
    pub max_entry_uncompressed_bytes: u64,
    /// 패키지 전체 엔트리의 최대 압축 해제 크기 합계.
    pub max_total_uncompressed_bytes: u64,
    /// XML 계열 엔트리 하나의 최대 압축 해제 크기.
    pub max_xml_uncompressed_bytes: u64,
    /// `압축 해제 크기 / 압축 크기`의 최대 허용 배율.
    pub max_compression_ratio: u64,
}

/// 외부 도구가 그대로 직렬화하여 동기화할 수 있는 기본 프로파일.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PackageLimitsProfile {
    pub profile: &'static str,
    pub rationale: &'static str,
    pub limits: PackageLimits,
}

/// hwp-cli가 읽기·검증·patch·쓰기에 공통 적용하는 단일 기본값.
///
/// 이름 하나는 ZIP 형식의 최대 범위(64 KiB)를 허용하되, 이름 합계는 16 MiB로
/// 제한한다. 4,096개 엔트리, 엔트리당 512 MiB, 전체 2 GiB, XML당 64 MiB는
/// 정상적인 대형 이미지 문서를 수용하면서 선언 크기 기반 ZIP bomb를 일찍 차단한다.
pub const NATIVE_PACKAGE_LIMITS: PackageLimits = PackageLimits {
    max_entries: 4_096,
    max_entry_name_bytes: 64 * 1024,
    max_total_name_bytes: 16 * 1024 * 1024,
    reject_duplicate_names: true,
    max_entry_uncompressed_bytes: 512 * 1024 * 1024,
    max_total_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
    max_xml_uncompressed_bytes: 64 * 1024 * 1024,
    max_compression_ratio: 1_000,
};

pub const NATIVE_PACKAGE_LIMITS_PROFILE: PackageLimitsProfile = PackageLimitsProfile {
    profile: NATIVE_PACKAGE_LIMITS_PROFILE_NAME,
    rationale: NATIVE_PACKAGE_LIMITS_RATIONALE,
    limits: NATIVE_PACKAGE_LIMITS,
};

impl Default for PackageLimits {
    fn default() -> Self {
        NATIVE_PACKAGE_LIMITS
    }
}

impl PackageLimits {
    fn check_name_len(&self, name_len: u64) -> Result<()> {
        if name_len > self.max_entry_name_bytes {
            return Err(limit_error(format!(
                "엔트리 이름 길이가 제한을 초과합니다 \
                 ({name_len} > {} 바이트)",
                self.max_entry_name_bytes
            )));
        }
        Ok(())
    }

    fn add_name_total(&self, total: u64, name_len: u64) -> Result<u64> {
        let next = total.checked_add(name_len).ok_or_else(|| {
            limit_error("엔트리 이름 길이 합계가 정수 범위를 넘었습니다".to_string())
        })?;
        if next > self.max_total_name_bytes {
            return Err(limit_error(format!(
                "엔트리 이름 길이 합계가 제한을 초과합니다 \
                 ({next} > {} 바이트)",
                self.max_total_name_bytes
            )));
        }
        Ok(next)
    }

    pub(crate) fn entry_uncompressed_limit(&self, name: &str) -> u64 {
        if is_xml_entry(name) {
            self.max_entry_uncompressed_bytes
                .min(self.max_xml_uncompressed_bytes)
        } else {
            self.max_entry_uncompressed_bytes
        }
    }

    pub(crate) fn check_entry(
        &self,
        name: &str,
        size: u64,
        compressed_size: Option<u64>,
    ) -> Result<()> {
        if size > self.max_entry_uncompressed_bytes {
            return Err(limit_error(format!(
                "'{name}' 엔트리의 압축 해제 크기가 제한을 초과합니다 \
                 ({size} > {} 바이트)",
                self.max_entry_uncompressed_bytes
            )));
        }
        if is_xml_entry(name) && size > self.max_xml_uncompressed_bytes {
            return Err(limit_error(format!(
                "'{name}' XML 엔트리 크기가 제한을 초과합니다 \
                 ({size} > {} 바이트)",
                self.max_xml_uncompressed_bytes
            )));
        }
        if let Some(compressed) = compressed_size {
            // raw copy도 무한정 큰 입력을 흘리지 않도록 같은 엔트리 예산으로 제한한다.
            if compressed > self.max_entry_uncompressed_bytes {
                return Err(limit_error(format!(
                    "'{name}' 엔트리의 압축 데이터 크기가 제한을 초과합니다 \
                     ({compressed} > {} 바이트)",
                    self.max_entry_uncompressed_bytes
                )));
            }
            if size > 0 && compressed == 0 {
                return Err(limit_error(format!(
                    "'{name}' 엔트리의 압축 크기가 0인데 압축 해제 크기는 {size}바이트입니다"
                )));
            }
            if compressed > 0 && size > compressed.saturating_mul(self.max_compression_ratio) {
                return Err(limit_error(format!(
                    "'{name}' 엔트리의 압축률이 제한을 초과합니다 \
                     ({size}/{compressed} > {}:1)",
                    self.max_compression_ratio
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn add_uncompressed_total(&self, total: u64, size: u64) -> Result<u64> {
        let next = total.checked_add(size).ok_or_else(|| {
            limit_error("패키지 압축 해제 크기 합계가 정수 범위를 넘었습니다".to_string())
        })?;
        if next > self.max_total_uncompressed_bytes {
            return Err(limit_error(format!(
                "패키지 압축 해제 크기 합계가 제한을 초과합니다 \
                 ({next} > {} 바이트)",
                self.max_total_uncompressed_bytes
            )));
        }
        Ok(next)
    }
}

/// ZIP 엔트리 메타데이터 (`hwp info`용).
#[derive(Debug, Clone)]
pub struct EntryInfo {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
}

pub struct HwpxPackage {
    zip: zip::ZipArchive<File>,
    entries: Vec<EntryInfo>,
    limits: PackageLimits,
    decrypted_entries: BTreeMap<String, zeroize::Zeroizing<Vec<u8>>>,
    decrypted_plaintext_bytes: u64,
    password_unlocked: bool,
    // Parser and document owners outlive the temporary decrypted overlay. This
    // is deliberately monotonic for one package invocation: it is a
    // conservative reservation for every returned byte buffer/string rather
    // than a best-effort estimate that could undercount a retained Document.
    parser_owned_plaintext_bytes: u64,
}

impl HwpxPackage {
    /// 파일을 열고 mimetype을 검증한다.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_limits(path, &PackageLimits::default())
    }

    /// 사용자 지정 자원 제한으로 파일을 열고 mimetype을 검증한다.
    pub fn open_with_limits(path: &Path, limits: &PackageLimits) -> Result<Self> {
        let mut file = File::open(path)?;
        preflight_central_directory(&mut file, limits)?;
        file.seek(SeekFrom::Start(0))?;
        let mut zip = zip::ZipArchive::new(file)?;
        let entries = inspect_archive(&mut zip, limits)?;
        let mut pkg = Self {
            zip,
            entries,
            limits: *limits,
            decrypted_entries: BTreeMap::new(),
            decrypted_plaintext_bytes: 0,
            password_unlocked: false,
            parser_owned_plaintext_bytes: 0,
        };
        let mime = pkg.read_entry_string("mimetype")?;
        if mime.trim() != MIMETYPE {
            return Err(HwpxError::BadMimetype(mime.trim().to_string()));
        }
        Ok(pkg)
    }

    /// 모든 엔트리를 보관 순서대로 열거한다.
    pub fn entries(&mut self) -> Result<Vec<EntryInfo>> {
        Ok(self.entries.clone())
    }

    pub fn read_entry(&mut self, name: &str) -> Result<Vec<u8>> {
        if let Some(plaintext_bytes) = self
            .decrypted_entries
            .get(name)
            .map(|plaintext| u64::try_from(plaintext.len()))
            .transpose()
            .map_err(|_| limit_error("암호화된 엔트리 크기가 정수 범위를 넘었습니다".to_string()))?
        {
            self.reserve_parser_owned_bytes(plaintext_bytes)?;
            // Keep the authenticated overlay available for later reads of the
            // same package entry. The returned copy is charged to the
            // monotonic parser/document ownership budget before allocation.
            return self
                .decrypted_entries
                .get(name)
                .map(|plaintext| plaintext.to_vec())
                .ok_or_else(|| HwpxError::EntryNotFound(name.to_string()));
        }
        let info = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| HwpxError::EntryNotFound(name.to_string()))?;
        let limit = self.limits.entry_uncompressed_limit(name);
        let expected_size = info.size;
        self.reserve_parser_owned_bytes(expected_size)?;
        let mut e = self
            .zip
            .by_name(name)
            .map_err(|_| HwpxError::EntryNotFound(name.to_string()))?;
        read_bounded(&mut e, name, expected_size, limit)
    }

    pub fn read_entry_string(&mut self, name: &str) -> Result<String> {
        let bytes = self.read_entry(name)?;
        match String::from_utf8(bytes) {
            // String takes ownership of Vec's allocation, so the reservation
            // made by read_entry already covers this parser owner.
            Ok(text) => Ok(text),
            Err(error) => {
                let bytes = error.into_bytes();
                // Lossy UTF-8 conversion can allocate up to three bytes per
                // invalid input byte for U+FFFD. Reserve that copy before it
                // exists; keeping the reservation avoids an untracked string
                // retained by a parser or returned Document.
                let lossy_copy = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|size| size.checked_mul(3))
                    .ok_or_else(|| {
                        limit_error("문자열 변환 예약값이 정수 범위를 넘었습니다".to_string())
                    })?;
                self.reserve_parser_owned_bytes(lossy_copy)?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
        }
    }

    /// 디렉터리를 제외한 모든 엔트리를 제한 안에서 EOF까지 스트리밍한다.
    ///
    /// 바이트를 보관하지 않으면서 deflate 오류와 CRC 불일치를 검출한다. 중앙
    /// 디렉터리 선언만 검사해서는 raw-copy 대상 `BinData/*` 손상을 알 수 없으므로,
    /// 구조 검증과 보존 patch는 이 메서드를 먼저 통과해야 한다.
    pub fn verify_integrity(&mut self) -> Result<()> {
        verify_archive_integrity(&mut self.zip, &self.entries, &self.limits)
    }

    /// Unlocks the observed, evidence-bound ODF profile into a per-package
    /// in-memory overlay. Plaintext never becomes a ZIP or a file on disk.
    pub fn unlock_with_password(&mut self, password: &str) -> Result<()> {
        if self.password_unlocked {
            return Err(HwpxError::Encrypted);
        }
        let manifest = self
            .read_entry_string("META-INF/manifest.xml")
            .map_err(|_| HwpxError::Encrypted)?;
        let profile = password::parse_profile(&manifest)?;
        let mut retained_plaintext = self.live_plaintext_bytes()?;
        let mut overlay = BTreeMap::new();
        for entry in profile.entries() {
            let info = self
                .entries
                .iter()
                .find(|candidate| candidate.name == entry.name)
                .ok_or(HwpxError::Encrypted)?;
            let stored = {
                let raw = self
                    .zip
                    .by_name(&entry.name)
                    .map_err(|_| HwpxError::Encrypted)?;
                raw.compression() == zip::CompressionMethod::Stored
            };
            if !stored || info.compressed_size != info.size {
                return Err(HwpxError::Encrypted);
            }
            validate_protected_ciphertext_read(
                retained_plaintext,
                info.size,
                self.limits.max_total_uncompressed_bytes,
            )?;
            let ciphertext = self.read_entry_untracked(&entry.name)?;
            let plaintext = password::decrypt_entry(
                entry,
                password,
                ciphertext,
                &self.limits,
                retained_plaintext,
            )?;
            retained_plaintext = retained_plaintext
                .checked_add(plaintext.len() as u64)
                .filter(|total| *total <= self.limits.max_total_uncompressed_bytes)
                .ok_or(HwpxError::Encrypted)?;
            overlay.insert(entry.name.clone(), plaintext);
        }
        self.decrypted_entries = overlay;
        self.decrypted_plaintext_bytes = retained_plaintext
            .checked_sub(self.parser_owned_plaintext_bytes)
            .ok_or(HwpxError::Encrypted)?;
        self.password_unlocked = true;
        Ok(())
    }

    /// Whether this package has authenticated a password-protected profile.
    ///
    /// Readers use this to avoid preserving source encryption metadata when
    /// the returned document now owns plaintext entries.
    pub fn was_unlocked_with_password(&self) -> bool {
        self.password_unlocked
    }

    fn read_entry_untracked(&mut self, name: &str) -> Result<Vec<u8>> {
        let info = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| HwpxError::EntryNotFound(name.to_string()))?;
        let limit = self.limits.entry_uncompressed_limit(name);
        let expected_size = info.size;
        let mut entry = self
            .zip
            .by_name(name)
            .map_err(|_| HwpxError::EntryNotFound(name.to_string()))?;
        read_bounded(&mut entry, name, expected_size, limit)
    }

    fn live_plaintext_bytes(&self) -> Result<u64> {
        self.decrypted_plaintext_bytes
            .checked_add(self.parser_owned_plaintext_bytes)
            .ok_or_else(|| limit_error("패키지 메모리 예약값이 정수 범위를 넘었습니다".to_string()))
    }

    fn reserve_parser_owned_bytes(&mut self, bytes: u64) -> Result<()> {
        let parser_owned = self
            .parser_owned_plaintext_bytes
            .checked_add(bytes)
            .ok_or_else(|| {
                limit_error("파서 메모리 예약값이 정수 범위를 넘었습니다".to_string())
            })?;
        let live = self
            .decrypted_plaintext_bytes
            .checked_add(parser_owned)
            .ok_or_else(|| {
                limit_error("패키지 메모리 예약값이 정수 범위를 넘었습니다".to_string())
            })?;
        if live > self.limits.max_total_uncompressed_bytes {
            return Err(limit_error(
                "암호화된 엔트리의 파서/문서 메모리가 패키지 제한을 초과합니다".to_string(),
            ));
        }
        self.parser_owned_plaintext_bytes = parser_owned;
        Ok(())
    }

    /// Refuses an encrypted package before any content part is read (GATE-02, D-04/D-05).
    ///
    /// `META-INF/manifest.xml` stays plaintext even when every content part is
    /// AES-encrypted (ODF packaging convention), so detection needs no decryption: an
    /// `odf:file-entry` carrying a child `odf:encryption-data` element marks encryption.
    /// A normal package's manifest is the empty self-closing element the writer's own
    /// `write::templates::MANIFEST_XML` emits.
    ///
    /// Read through the existing bounded `read_entry_string`, so this entry is bounded
    /// exactly like every other (T-2-10). A missing manifest, an unparseable manifest, or
    /// the marker appearing only in a comment or as element text all leave reading
    /// unaffected — a manifest is not needed to read a document, and a detection
    /// heuristic on a non-essential entry must not block one that is otherwise fine
    /// (T-2-13, deliberate fail-open, recorded here per the plan's own requirement).
    /// Returns whether the plaintext manifest marks one or more package entries as
    /// encrypted. Missing or malformed manifests deliberately remain fail-open,
    /// matching [`Self::check_body_readable`]'s historic behavior.
    pub fn has_encryption_marker(&mut self) -> Result<bool> {
        let xml = match self.read_entry_string("META-INF/manifest.xml") {
            Ok(xml) => xml,
            Err(HwpxError::EntryNotFound(_)) => return Ok(false),
            // Any other error (a limit violation, an archive error) surfaces as itself.
            Err(other) => return Err(other),
        };
        let mut reader = quick_xml::Reader::from_str(&xml);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    // Match the element's local name, not a substring of the raw text,
                    // so a namespace prefix change does not defeat detection and a
                    // comment/text mention of the word does not trigger a false refusal.
                    if e.local_name().as_ref() == b"encryption-data" {
                        return Ok(true);
                    }
                }
                Ok(Event::Eof) => return Ok(false),
                Ok(_) => {}
                // Fail-open on a malformed manifest: see the doc comment above (T-2-13).
                Err(_) => return Ok(false),
            }
        }
    }

    pub fn check_body_readable(&mut self) -> Result<()> {
        if self.has_encryption_marker()? {
            Err(HwpxError::Encrypted)
        } else {
            Ok(())
        }
    }

    /// `version.xml` 루트 요소의 속성들을 (이름, 값) 쌍으로 반환한다.
    /// (스키마에 의존하지 않는 관용적 추출 — `hwp info` 표시용)
    pub fn version_info(&mut self) -> Result<Vec<(String, String)>> {
        let xml = self.read_entry_string("version.xml")?;
        let mut reader = quick_xml::Reader::from_str(&xml);
        loop {
            match reader.read_event().map_err(|e| HwpxError::Xml {
                entry: "version.xml".to_string(),
                message: e.to_string(),
            })? {
                Event::Start(e) | Event::Empty(e) => {
                    let mut attrs = Vec::new();
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                        let value = attr
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .unwrap_or_default()
                            .into_owned();
                        attrs.push((key, value));
                    }
                    return Ok(attrs);
                }
                Event::Eof => return Ok(Vec::new()),
                _ => {}
            }
        }
    }

    /// 본문 섹션 엔트리 이름 목록 (`Contents/section0.xml`, …).
    pub fn section_entries(&mut self) -> Result<Vec<String>> {
        let mut v: Vec<String> = self
            .entries()?
            .into_iter()
            .map(|e| e.name)
            .filter(|n| n.starts_with("Contents/section") && n.ends_with(".xml"))
            .collect();
        v.sort_by_key(|n| {
            n.trim_start_matches("Contents/section")
                .trim_end_matches(".xml")
                .parse::<u32>()
                .unwrap_or(u32::MAX)
        });
        Ok(v)
    }
}

fn validate_protected_ciphertext_read(
    retained_plaintext: u64,
    ciphertext_bytes: u64,
    live_limit: u64,
) -> Result<()> {
    retained_plaintext
        .checked_add(ciphertext_bytes)
        .filter(|live| *live <= live_limit)
        .map(|_| ())
        .ok_or(HwpxError::Encrypted)
}

/// zip 크레이트는 중앙 디렉터리를 이름-keyed map으로 만들며 중복 엔트리를 덮어쓴다.
/// 따라서 `ZipArchive` 생성 전에 원시 중앙 디렉터리의 개수·중복·크기를 검사한다.
pub(crate) fn preflight_central_directory<R: Read + Seek>(
    reader: &mut R,
    limits: &PackageLimits,
) -> Result<()> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    let tail_len = file_len.min(22 + u64::from(u16::MAX)) as usize;
    reader.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0u8; tail_len];
    reader.read_exact(&mut tail)?;
    if tail.len() < 22 {
        return Err(integrity_error(
            "ZIP EOCD 레코드를 담기에는 파일이 너무 짧습니다".to_string(),
        ));
    }

    let eocd_at = (0..=tail.len() - 22).rev().find(|&offset| {
        &tail[offset..offset + 4] == b"PK\x05\x06"
            && offset + 22 + usize::from(le_u16(&tail, offset + 20)) == tail.len()
    });
    let Some(eocd_at) = eocd_at else {
        return Err(integrity_error(
            "파일 끝과 정확히 맞는 ZIP EOCD 레코드가 없습니다 \
             (잘못된 comment 길이 또는 후행 데이터)"
                .to_string(),
        ));
    };
    let eocd_pos = file_len - tail_len as u64 + eocd_at as u64;
    let eocd = &tail[eocd_at..];
    let mut entry_count = u64::from(le_u16(eocd, 10));
    let mut central_size = u64::from(le_u32(eocd, 12));
    let mut central_offset = u64::from(le_u32(eocd, 16));
    let mut central_end = eocd_pos;

    if entry_count == u64::from(u16::MAX)
        || central_size == u64::from(u32::MAX)
        || central_offset == u64::from(u32::MAX)
    {
        let locator_pos = eocd_pos.checked_sub(20).ok_or_else(|| {
            integrity_error("ZIP64 중앙 디렉터리 locator 위치가 잘못되었습니다".to_string())
        })?;
        reader.seek(SeekFrom::Start(locator_pos))?;
        let mut locator = [0u8; 20];
        reader.read_exact(&mut locator)?;
        if &locator[..4] != b"PK\x06\x07" {
            return Err(integrity_error(
                "ZIP64 중앙 디렉터리 locator가 없습니다".to_string(),
            ));
        }
        let zip64_pos = le_u64(&locator, 8);
        reader.seek(SeekFrom::Start(zip64_pos))?;
        let mut zip64 = [0u8; 56];
        reader.read_exact(&mut zip64)?;
        if &zip64[..4] != b"PK\x06\x06" {
            return Err(integrity_error(
                "ZIP64 중앙 디렉터리 레코드가 없습니다".to_string(),
            ));
        }
        entry_count = le_u64(&zip64, 32);
        central_size = le_u64(&zip64, 40);
        central_offset = le_u64(&zip64, 48);
        central_end = zip64_pos;
    }

    if entry_count > limits.max_entries as u64 {
        return Err(limit_error(format!(
            "엔트리 수가 제한을 초과합니다 ({entry_count} > {})",
            limits.max_entries
        )));
    }

    let declared_central_end = central_offset.checked_add(central_size).ok_or_else(|| {
        integrity_error("중앙 디렉터리 위치가 정수 범위를 넘었습니다".to_string())
    })?;
    if declared_central_end > central_end {
        return Err(integrity_error(
            "중앙 디렉터리의 선언 위치가 EOCD 뒤를 가리킵니다".to_string(),
        ));
    }

    // 자체압축해제 ZIP처럼 앞에 stub이 있으면 중앙 디렉터리 offset이 archive 기준일
    // 수 있다. 선언 위치에 시그니처가 없을 때 실제 끝 위치에서 역산한다.
    let mut central_start = central_offset;
    if entry_count > 0 && !has_signature(reader, central_start, b"PK\x01\x02")? {
        central_start = central_end
            .checked_sub(central_size)
            .ok_or_else(|| integrity_error("중앙 디렉터리 크기와 위치가 모순됩니다".to_string()))?;
        if !has_signature(reader, central_start, b"PK\x01\x02")? {
            return Err(integrity_error(
                "중앙 디렉터리 시작 레코드가 없습니다".to_string(),
            ));
        }
    }
    let expected_central_end = central_start.checked_add(central_size).ok_or_else(|| {
        integrity_error("중앙 디렉터리 크기가 정수 범위를 넘었습니다".to_string())
    })?;
    if expected_central_end != central_end {
        return Err(integrity_error(format!(
            "중앙 디렉터리 끝과 EOCD 위치가 맞지 않습니다 \
             ({expected_central_end} != {central_end})"
        )));
    }
    reader.seek(SeekFrom::Start(central_start))?;

    let mut names: BTreeSet<Box<[u8]>> = BTreeSet::new();
    let mut name_total = 0u64;
    let mut total = 0u64;
    let mut compressed_total = 0u64;
    for _ in 0..entry_count {
        let mut fixed = [0u8; 46];
        reader.read_exact(&mut fixed)?;
        if &fixed[..4] != b"PK\x01\x02" {
            return Err(integrity_error(
                "중앙 디렉터리 엔트리 레코드가 잘못되었습니다".to_string(),
            ));
        }
        let name_len = u64::from(le_u16(&fixed, 28));
        let extra_len = usize::from(le_u16(&fixed, 30));
        let comment_len = u64::from(le_u16(&fixed, 32));
        // zip 크레이트가 중앙 디렉터리 이름을 소유 버퍼로 만들기 전에 원시
        // 디렉터리를 직접 읽어 검사한다. 개별·합계 제한 모두 할당 전에 적용한다.
        limits.check_name_len(name_len)?;
        name_total = limits.add_name_total(name_total, name_len)?;
        let mut name = vec![0u8; name_len as usize];
        reader.read_exact(&mut name)?;
        validate_entry_name(&name)?;
        let mut extra = vec![0u8; extra_len];
        reader.read_exact(&mut extra)?;
        reader.seek(SeekFrom::Current(comment_len as i64))?;

        if limits.reject_duplicate_names && !names.insert(name.clone().into_boxed_slice()) {
            return Err(limit_error(format!(
                "중복 엔트리 이름을 허용하지 않습니다: '{}'",
                String::from_utf8_lossy(&name)
            )));
        }

        let mut size = u64::from(le_u32(&fixed, 24));
        let mut compressed_size = u64::from(le_u32(&fixed, 20));
        if size == u64::from(u32::MAX) || compressed_size == u64::from(u32::MAX) {
            let (zip64_size, zip64_compressed) = zip64_entry_sizes(
                &extra,
                size == u64::from(u32::MAX),
                compressed_size == u64::from(u32::MAX),
            );
            if let Some(value) = zip64_size {
                size = value;
            }
            if let Some(value) = zip64_compressed {
                compressed_size = value;
            }
        }
        let display_name = String::from_utf8_lossy(&name);
        limits.check_entry(&display_name, size, Some(compressed_size))?;
        total = limits.add_uncompressed_total(total, size)?;
        compressed_total = compressed_total
            .checked_add(compressed_size)
            .ok_or_else(|| {
                limit_error("패키지 압축 데이터 크기 합계가 정수 범위를 넘었습니다".to_string())
            })?;
        if compressed_total > limits.max_total_uncompressed_bytes {
            return Err(limit_error(format!(
                "패키지 압축 데이터 크기 합계가 제한을 초과합니다 \
                 ({compressed_total} > {} 바이트)",
                limits.max_total_uncompressed_bytes
            )));
        }
    }
    let actual_central_end = reader.stream_position()?;
    if actual_central_end != expected_central_end {
        return Err(integrity_error(format!(
            "중앙 디렉터리 선언 크기와 실제 엔트리 범위가 다릅니다 \
             ({actual_central_end} != {expected_central_end})"
        )));
    }
    Ok(())
}

/// 중앙 디렉터리의 선언값을 전부 검사한다. 어떤 엔트리도 압축 해제하지 않는다.
pub(crate) fn inspect_archive<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    limits: &PackageLimits,
) -> Result<Vec<EntryInfo>> {
    if zip.len() > limits.max_entries {
        return Err(limit_error(format!(
            "엔트리 수가 제한을 초과합니다 ({} > {})",
            zip.len(),
            limits.max_entries
        )));
    }

    let mut entries = Vec::with_capacity(zip.len());
    let mut names: BTreeSet<Box<[u8]>> = BTreeSet::new();
    let mut name_total = 0u64;
    let mut total = 0u64;
    let mut compressed_total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        let raw_name = entry.name_raw();
        limits.check_name_len(raw_name.len() as u64)?;
        name_total = limits.add_name_total(name_total, raw_name.len() as u64)?;
        validate_entry_name(raw_name)?;
        let name = entry.name().to_string();
        if limits.reject_duplicate_names && !names.insert(raw_name.into()) {
            return Err(limit_error(format!(
                "중복 엔트리 이름을 허용하지 않습니다: '{name}'"
            )));
        }
        let size = entry.size();
        let compressed_size = entry.compressed_size();
        limits.check_entry(&name, size, Some(compressed_size))?;
        total = limits.add_uncompressed_total(total, size)?;
        compressed_total = compressed_total
            .checked_add(compressed_size)
            .ok_or_else(|| {
                limit_error("패키지 압축 데이터 크기 합계가 정수 범위를 넘었습니다".to_string())
            })?;
        if compressed_total > limits.max_total_uncompressed_bytes {
            return Err(limit_error(format!(
                "패키지 압축 데이터 크기 합계가 제한을 초과합니다 \
                 ({compressed_total} > {} 바이트)",
                limits.max_total_uncompressed_bytes
            )));
        }
        entries.push(EntryInfo {
            name,
            size,
            compressed_size,
        });
    }
    Ok(entries)
}

/// 쓰기 전에 생성할 엔트리 이름과 크기를 동일 정책으로 검사한다.
pub(crate) fn validate_output_entries<I, S>(entries: I, limits: &PackageLimits) -> Result<()>
where
    I: IntoIterator<Item = (S, u64)>,
    S: AsRef<str>,
{
    let mut count = 0usize;
    let mut names = BTreeSet::new();
    let mut name_total = 0u64;
    let mut total = 0u64;
    for (name, size) in entries {
        count = count
            .checked_add(1)
            .ok_or_else(|| limit_error("엔트리 수가 정수 범위를 넘었습니다".to_string()))?;
        if count > limits.max_entries {
            return Err(limit_error(format!(
                "엔트리 수가 제한을 초과합니다 ({count} > {})",
                limits.max_entries
            )));
        }
        let name = name.as_ref();
        limits.check_name_len(name.len() as u64)?;
        name_total = limits.add_name_total(name_total, name.len() as u64)?;
        validate_entry_name(name.as_bytes())?;
        if limits.reject_duplicate_names && !names.insert(name.to_string()) {
            return Err(limit_error(format!(
                "중복 엔트리 이름을 허용하지 않습니다: '{name}'"
            )));
        }
        limits.check_entry(name, size, None)?;
        total = limits.add_uncompressed_total(total, size)?;
    }
    Ok(())
}

/// 중앙 디렉터리 검사를 마친 모든 비디렉터리 엔트리를 EOF까지 읽어 실제 압축
/// 스트림과 CRC를 검증한다. `std::io::sink`로 흘리므로 BinData 크기에 비례한
/// 메모리를 사용하지 않는다.
pub(crate) fn verify_archive_integrity<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    entries: &[EntryInfo],
    limits: &PackageLimits,
) -> Result<()> {
    for (index, info) in entries.iter().enumerate() {
        let mut entry = zip.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let limit = limits.entry_uncompressed_limit(&info.name);
        stream_bounded(&mut entry, &info.name, info.size, limit)?;
    }
    Ok(())
}

/// 엔트리를 보관하지 않고 EOF까지 읽는다. EOF 읽기가 zip 크레이트의 CRC 검사를
/// 발동시키므로, 선언 크기만 맞춘 손상 파일도 통과하지 못한다.
pub(crate) fn stream_bounded(
    reader: &mut impl Read,
    name: &str,
    expected_size: u64,
    limit: u64,
) -> Result<u64> {
    if expected_size > limit {
        return Err(limit_error(format!(
            "'{name}' 엔트리 크기가 읽기 제한을 초과합니다 \
             ({expected_size} > {limit} 바이트)"
        )));
    }
    let actual = std::io::copy(
        &mut reader.take(limit.saturating_add(1)),
        &mut std::io::sink(),
    )
    .map_err(|error| {
        integrity_error(format!("'{name}' 엔트리를 끝까지 읽지 못했습니다: {error}"))
    })?;
    if actual > limit {
        return Err(limit_error(format!(
            "'{name}' 엔트리의 실제 압축 해제 크기가 제한을 초과합니다 \
             ({actual} > {limit} 바이트)"
        )));
    }
    if actual != expected_size {
        return Err(integrity_error(format!(
            "'{name}' 엔트리의 실제 압축 해제 크기가 선언과 다릅니다 \
             ({actual} != {expected_size} 바이트)"
        )));
    }
    Ok(actual)
}

/// 선언 크기 검사 후 실제 압축 해제 바이트도 `limit + 1`로 감싸 이중 방어한다.
pub(crate) fn read_bounded(
    reader: &mut impl Read,
    name: &str,
    expected_size: u64,
    limit: u64,
) -> Result<Vec<u8>> {
    if expected_size > limit {
        return Err(limit_error(format!(
            "'{name}' 엔트리 크기가 읽기 제한을 초과합니다 \
             ({expected_size} > {limit} 바이트)"
        )));
    }
    // 거짓 중앙 디렉터리 크기로 큰 선할당을 유도하지 못하게 초기 예약은 작게 제한한다.
    let initial = expected_size.min(64 * 1024) as usize;
    let mut data = Vec::with_capacity(initial);
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|error| {
            integrity_error(format!("'{name}' 엔트리를 끝까지 읽지 못했습니다: {error}"))
        })?;
    if data.len() as u64 > limit {
        return Err(limit_error(format!(
            "'{name}' 엔트리의 실제 압축 해제 크기가 제한을 초과합니다 \
             ({} > {limit} 바이트)",
            data.len()
        )));
    }
    if data.len() as u64 != expected_size {
        return Err(integrity_error(format!(
            "'{name}' 엔트리의 실제 압축 해제 크기가 선언과 다릅니다 \
             ({} != {expected_size} 바이트)",
            data.len()
        )));
    }
    Ok(data)
}

/// HWPX/OPC part 이름은 UTF-8 상대 경로만 허용한다. 절대 경로, Windows
/// 구분자, NUL, 빈/현재/상위 컴포넌트를 중앙 디렉터리 단계에서 거부해 읽기·patch·
/// 쓰기가 같은 이름 의미를 사용하게 한다.
fn validate_entry_name(raw_name: &[u8]) -> Result<()> {
    let name = std::str::from_utf8(raw_name)
        .map_err(|_| limit_error("엔트리 이름이 유효한 UTF-8이 아닙니다".to_string()))?;
    if name.is_empty() {
        return Err(limit_error(
            "빈 엔트리 이름을 허용하지 않습니다".to_string(),
        ));
    }
    if name.contains('\0') {
        return Err(limit_error(format!(
            "NUL을 포함한 엔트리 이름을 허용하지 않습니다: {name:?}"
        )));
    }
    if name.starts_with('/') || name.starts_with('\\') || name.contains('\\') {
        return Err(limit_error(format!(
            "절대 경로 또는 역슬래시 엔트리 이름을 허용하지 않습니다: {name:?}"
        )));
    }

    let path = name.strip_suffix('/').unwrap_or(name);
    if path.is_empty() {
        return Err(limit_error(format!(
            "루트 디렉터리 엔트리 이름을 허용하지 않습니다: {name:?}"
        )));
    }
    let mut components = path.split('/');
    if components.next().is_some_and(is_windows_drive_prefix) {
        return Err(limit_error(format!(
            "드라이브 절대 경로 엔트리 이름을 허용하지 않습니다: {name:?}"
        )));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(limit_error(format!(
            "빈/현재/상위 경로 컴포넌트를 허용하지 않습니다: {name:?}"
        )));
    }
    Ok(())
}

fn is_windows_drive_prefix(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_xml_entry(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        ext.eq_ignore_ascii_case("xml")
            || ext.eq_ignore_ascii_case("hpf")
            || ext.eq_ignore_ascii_case("rdf")
    })
}

fn limit_error(message: String) -> HwpxError {
    HwpxError::PackageLimit(message)
}

fn integrity_error(message: String) -> HwpxError {
    HwpxError::PackageIntegrity(message)
}

fn has_signature(
    reader: &mut (impl Read + Seek),
    offset: u64,
    signature: &[u8; 4],
) -> std::io::Result<bool> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut actual = [0u8; 4];
    Ok(reader.read_exact(&mut actual).is_ok() && &actual == signature)
}

fn zip64_entry_sizes(
    extra: &[u8],
    need_size: bool,
    need_compressed: bool,
) -> (Option<u64>, Option<u64>) {
    let mut cursor = 0usize;
    while cursor + 4 <= extra.len() {
        let tag = le_u16(extra, cursor);
        let len = usize::from(le_u16(extra, cursor + 2));
        cursor += 4;
        let Some(end) = cursor.checked_add(len).filter(|end| *end <= extra.len()) else {
            break;
        };
        if tag == 0x0001 {
            let mut field = &extra[cursor..end];
            let size = if need_size && field.len() >= 8 {
                let value = le_u64(field, 0);
                field = &field[8..];
                Some(value)
            } else {
                None
            };
            let compressed = if need_compressed && field.len() >= 8 {
                Some(le_u64(field, 0))
            } else {
                None
            };
            return (size, compressed);
        }
        cursor = end;
    }
    (None, None)
}

fn le_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    //! CI-safe: synthetic in-memory-built ZIPs only, no corpus, no committed fixture.

    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temp_file(label: &str) -> PathBuf {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "hwpx_encrypted_gate_{label}_{}_{}.hwpx",
            std::process::id(),
            id
        ))
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, data) in entries {
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// Opens a package built from `mimetype` plus an optional `META-INF/manifest.xml`
    /// body, asserting `check_body_readable()`'s outcome.
    fn open_and_check(label: &str, manifest: Option<&[u8]>) -> Result<()> {
        let path = temp_file(label);
        let mut entries: Vec<(&str, &[u8])> = vec![(MIMETYPE_ENTRY, MIMETYPE.as_bytes())];
        if let Some(body) = manifest {
            entries.push(("META-INF/manifest.xml", body));
        }
        write_zip(&path, &entries);
        let mut pkg = HwpxPackage::open(&path).unwrap();
        let result = pkg.check_body_readable();
        let _ = std::fs::remove_file(&path);
        result
    }

    const MIMETYPE_ENTRY: &str = "mimetype";

    const ENCRYPTED_MANIFEST: &str = r##"<?xml version="1.0" encoding="UTF-8"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/header.xml" media-type="application/xml" size="1"><odf:encryption-data checksum-type="...sha256-1k" checksum="..."><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/></odf:encryption-data></odf:file-entry></odf:manifest>"##;

    #[test]
    fn manifest_with_an_encryption_element_refuses() {
        let result = open_and_check("encrypted", Some(ENCRYPTED_MANIFEST.as_bytes()));
        assert!(matches!(result, Err(HwpxError::Encrypted)), "{result:?}");
    }

    #[test]
    fn the_writers_own_empty_manifest_is_readable() {
        let result = open_and_check(
            "empty_manifest",
            Some(crate::write::templates::MANIFEST_XML.as_bytes()),
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn a_package_without_a_manifest_entry_is_readable() {
        let result = open_and_check("no_manifest", None);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn an_unparseable_manifest_does_not_block_reading() {
        let result = open_and_check(
            "unparseable",
            Some(b"<odf:manifest><odf:file-entry not closed"),
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn the_marker_word_in_a_comment_is_not_a_refusal() {
        let manifest = br##"<?xml version="1.0" encoding="UTF-8"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><!-- encryption-data is mentioned here only as a comment --><odf:file-entry full-path="Contents/header.xml" media-type="application/xml" size="1"><odf:comment>encryption-data</odf:comment></odf:file-entry></odf:manifest>"##;
        let result = open_and_check("comment_marker", Some(manifest));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn retained_parser_entries_reject_the_next_lossy_string_before_its_copy() {
        let path = temp_file("parser-live-budget");
        write_zip(
            &path,
            &[
                (MIMETYPE_ENTRY, MIMETYPE.as_bytes()),
                ("Contents/first.bin", b"placeholder-first"),
                ("Contents/second.bin", b"placeholder-second"),
            ],
        );
        let limits = PackageLimits {
            // 48 bytes stay in the authenticated overlay. The first returned
            // copy raises live ownership to 72; the second raw copy raises it
            // to 96; its 72-byte lossy UTF-8 allocation must fail at 168.
            max_total_uncompressed_bytes: 120,
            ..PackageLimits::default()
        };
        let mut package = HwpxPackage::open_with_limits(&path, &limits).unwrap();
        // open() only needs mimetype during construction, so it is no longer a
        // live parser owner. Inject two equal synthetic protected entries to
        // exercise the package-local accounting without a corpus fixture.
        package.parser_owned_plaintext_bytes = 0;
        package.decrypted_entries.insert(
            "Contents/first.bin".to_string(),
            zeroize::Zeroizing::new(vec![b'a'; 24]),
        );
        package.decrypted_entries.insert(
            "Contents/second.bin".to_string(),
            zeroize::Zeroizing::new(vec![0xff; 24]),
        );
        package.decrypted_plaintext_bytes = 48;

        let first = package.read_entry("Contents/first.bin").unwrap();
        assert_eq!(first.len(), 24);
        let error = package
            .read_entry_string("Contents/second.bin")
            .unwrap_err();
        assert!(matches!(error, HwpxError::PackageLimit(_)), "{error:?}");
        assert_eq!(package.decrypted_plaintext_bytes, 48);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn protected_ciphertext_is_reserved_before_its_read_allocation() {
        assert!(validate_protected_ciphertext_read(80, 40, 120).is_ok());
        assert!(matches!(
            validate_protected_ciphertext_read(81, 40, 120),
            Err(HwpxError::Encrypted)
        ));
        assert!(matches!(
            validate_protected_ciphertext_read(u64::MAX, 1, u64::MAX),
            Err(HwpxError::Encrypted)
        ));
    }
}
