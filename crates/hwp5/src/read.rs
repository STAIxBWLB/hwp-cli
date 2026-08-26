//! 최상위: HWP 5.0 파일 → [`Document`].

use std::collections::BTreeMap;
use std::path::Path;

use hwp_model::{DocMeta, Document};
use zeroize::Zeroize as _;

use crate::body_text::parse_section;
use crate::container::{Hwp5Container, is_record_stream};
use crate::doc_info::parse_doc_info;
use crate::error::{Hwp5Error, Result};
use crate::file_header::FileHeader;
use crate::record::{RecordHeader, ScanMode, scan_stream};

mod password;
use password::{
    HWP5_PASSWORD_MAX_STREAM_BYTES, HWP5_PASSWORD_MAX_STREAMS,
    HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES, decrypt_hwp5_encrypt_version_4_in_place,
    validate_live_bytes,
};

/// Per-call options for HWP5 reads. Password bytes are borrowed only for the
/// active read and are never retained by the reader or returned document.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions<'a> {
    pub password: Option<&'a str>,
}

pub struct ReadResult {
    pub document: Document,
    /// 파싱 중 발생한 비치명 경고 (손상/미지원 구조)
    pub warnings: Vec<String>,
    /// Whether this read decrypted a Hancom distribution document (배포용문서)
    /// on the way in. A transient read-time signal for the CLI (D-02/D-03) —
    /// deliberately not part of the serialized IR, since it has no reason to
    /// survive past the caller's stderr notice.
    pub unwrapped_distribution: bool,
}

/// Limits used by certification before any compressed HWP5 stream is exposed
/// to the semantic parser. Stored and materialized bytes are independently
/// bounded because a small raw-DEFLATE stream can expand by orders of magnitude.
#[derive(Debug, Clone, Copy)]
pub struct BoundedReadLimits {
    pub max_streams: usize,
    pub max_total_stream_name_bytes: u64,
    pub max_stream_bytes: u64,
    pub max_total_materialized_bytes: u64,
    pub max_records: usize,
    pub max_record_depth: usize,
}

/// Immutable, already-bounded HWP5 stream snapshot. Repeated semantic imports
/// reuse these exact bytes and never reopen or decompress the source file.
#[derive(Debug)]
pub struct BoundedReadSnapshot {
    header: FileHeader,
    streams: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptPresence {
    Absent,
    Present,
    Indeterminate,
}

impl BoundedReadSnapshot {
    pub fn open(path: &Path, limits: BoundedReadLimits) -> Result<Self> {
        let mut container = Hwp5Container::open(path)?;
        container.check_body_readable()?;
        // This is a certification-oriented raw-stream cache, not the main read
        // path (GATE-01's ViewText decrypt lives in `read_document` only).
        // Without this guard a distribution document would fall through and
        // get snapshotted with its `/ViewText/` bytes still encrypted.
        if container.file_header().is_distribution() {
            return Err(Hwp5Error::DistributionDoc);
        }
        let header = container.file_header().clone();
        let compressed = header.is_compressed();
        let mut streams = BTreeMap::new();
        let mut total_materialized = 0u64;
        let mut total_records = 0usize;

        for info in container
            .list_streams_bounded(limits.max_streams, limits.max_total_stream_name_bytes)?
        {
            if info.size > limits.max_stream_bytes {
                return Err(Hwp5Error::ResourceLimitExceeded {
                    resource: format!("{} stored stream", info.path),
                    limit: limits.max_stream_bytes,
                });
            }
            let raw = container.read_stream_raw(&info.path)?;
            let materialized = if compressed && is_record_stream(&info.path) {
                crate::codec::decompress_bounded(&raw, &info.path, limits.max_stream_bytes)?
            } else if compressed && info.path.starts_with("/BinData/") {
                // BinData compression is recorded per BIN_DATA item rather than
                // by path. Preserve the legacy try-DEFLATE behavior, but only a
                // malformed stream may fall back to its already-bounded raw
                // bytes. Expansion-limit failures are always fatal.
                match crate::codec::decompress_bounded(&raw, &info.path, limits.max_stream_bytes) {
                    Ok(data) => data,
                    Err(Hwp5Error::Decompress { .. }) => raw,
                    Err(error) => return Err(error),
                }
            } else {
                raw
            };
            total_materialized = total_materialized
                .checked_add(materialized.len() as u64)
                .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                    resource: "aggregate materialized streams".to_string(),
                    limit: limits.max_total_materialized_bytes,
                })?;
            if total_materialized > limits.max_total_materialized_bytes {
                return Err(Hwp5Error::ResourceLimitExceeded {
                    resource: "aggregate materialized streams".to_string(),
                    limit: limits.max_total_materialized_bytes,
                });
            }
            if is_semantic_record_stream(&info.path) {
                validate_record_budget(
                    &materialized,
                    &mut total_records,
                    limits.max_records,
                    limits.max_record_depth,
                )?;
            }
            streams.insert(info.path, materialized);
        }

        Ok(Self { header, streams })
    }

    /// Parse the bounded stream cache. Calling this more than once repeats only
    /// semantic parsing; it does not perform filesystem I/O or decompression.
    pub fn read_document(&self) -> Result<ReadResult> {
        read_document_from_streams(&self.header, &self.streams)
    }

    /// Macro source presence derived from the same bounded Scripts cache.
    /// DefaultJScript consists of length-prefixed UTF-16LE source blocks,
    /// zero-length bookkeeping blocks, and a trailing `0xFFFF_FFFF` sentinel.
    pub fn script_presence(&self) -> ScriptPresence {
        let mut present = false;
        for (path, bytes) in &self.streams {
            if !path.starts_with("/Scripts/") || path.ends_with("/JScriptVersion") {
                continue;
            }
            match parse_script_source(bytes) {
                ScriptPresence::Present => present = true,
                ScriptPresence::Indeterminate => return ScriptPresence::Indeterminate,
                ScriptPresence::Absent => {}
            }
        }
        if present {
            ScriptPresence::Present
        } else {
            ScriptPresence::Absent
        }
    }
}

fn parse_script_source(bytes: &[u8]) -> ScriptPresence {
    let mut offset = 0usize;
    let mut nonempty_source = false;
    loop {
        let Some(raw_length) = bytes.get(offset..offset.saturating_add(4)) else {
            return ScriptPresence::Indeterminate;
        };
        let length = u32::from_le_bytes(raw_length.try_into().expect("four-byte length"));
        offset += 4;
        if length == u32::MAX {
            return if offset == bytes.len() {
                if nonempty_source {
                    ScriptPresence::Present
                } else {
                    ScriptPresence::Absent
                }
            } else {
                ScriptPresence::Indeterminate
            };
        }
        let Some(byte_length) = (length as usize).checked_mul(2) else {
            return ScriptPresence::Indeterminate;
        };
        let Some(end) = offset.checked_add(byte_length) else {
            return ScriptPresence::Indeterminate;
        };
        let Some(source_bytes) = bytes.get(offset..end) else {
            return ScriptPresence::Indeterminate;
        };
        for character in std::char::decode_utf16(
            source_bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
        ) {
            let Ok(character) = character else {
                return ScriptPresence::Indeterminate;
            };
            if character != '\0' && !character.is_whitespace() {
                nonempty_source = true;
            }
        }
        offset = end;
    }
}

fn is_semantic_record_stream(path: &str) -> bool {
    path == "/DocInfo" || path.starts_with("/BodyText/") || path.starts_with("/ViewText/")
}

fn validate_record_budget(
    data: &[u8],
    total_records: &mut usize,
    max_records: usize,
    max_depth: usize,
) -> Result<()> {
    let mut reader = crate::codec::ByteReader::new(data);
    while !reader.is_empty() {
        let header = RecordHeader::decode(&mut reader)?;
        *total_records =
            total_records
                .checked_add(1)
                .ok_or_else(|| Hwp5Error::StructureLimitExceeded {
                    resource: "aggregate record count".to_string(),
                    limit: max_records,
                })?;
        if *total_records > max_records {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "aggregate record count".to_string(),
                limit: max_records,
            });
        }
        if usize::from(header.level) > max_depth {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "record nesting depth".to_string(),
                limit: max_depth,
            });
        }
        reader.read_bytes(header.size as usize)?;
    }
    Ok(())
}

fn read_document_from_streams(
    file_header: &FileHeader,
    streams: &BTreeMap<String, Vec<u8>>,
) -> Result<ReadResult> {
    let mut warnings = Vec::new();
    let doc_info_data = streams
        .get("/DocInfo")
        .ok_or_else(|| Hwp5Error::StreamNotFound("/DocInfo".to_string()))?;
    let scan = scan_stream(doc_info_data, ScanMode::Tolerant)?;
    warnings.extend(
        scan.warnings
            .iter()
            .map(|warning| format!("[DocInfo] {warning}")),
    );
    let (header, doc_warnings) = parse_doc_info(&scan.roots);
    warnings.extend(
        doc_warnings
            .iter()
            .map(|warning| format!("[DocInfo] {warning}")),
    );

    let mut body_sections: Vec<&str> = streams
        .keys()
        .filter(|path| path.starts_with("/BodyText/Section"))
        .map(String::as_str)
        .collect();
    body_sections.sort_by_key(|path| {
        path.trim_start_matches("/BodyText/Section")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let mut sections = Vec::with_capacity(body_sections.len());
    for stream_path in body_sections {
        let data = &streams[stream_path];
        let scan = scan_stream(data, ScanMode::Tolerant)?;
        warnings.extend(
            scan.warnings
                .iter()
                .map(|warning| format!("[{stream_path}] {warning}")),
        );
        let (section, section_warnings) = parse_section(&scan.roots);
        warnings.extend(
            section_warnings
                .iter()
                .map(|warning| format!("[{stream_path}] {warning}")),
        );
        sections.push(section);
    }

    let bin_streams = streams
        .iter()
        .filter_map(|(path, data)| {
            path.strip_prefix("/BinData/")
                .map(|name| hwp_model::BinStream {
                    name: name.to_string(),
                    data: data.clone(),
                })
        })
        .collect();
    let metadata = streams
        .get("/\u{5}HwpSummaryInformation")
        .map(|raw| crate::summary::parse_summary(raw))
        .unwrap_or_default();
    let hwp5_xml_template = streams
        .iter()
        .filter(|(path, _)| path.starts_with("/XMLTemplate/"))
        .map(|(path, data)| (path.clone(), data.clone()))
        .collect();
    let hwp5_doc_history = streams
        .iter()
        .filter(|(path, _)| path.starts_with("/DocHistory/"))
        .map(|(path, data)| (path.clone(), data.clone()))
        .collect();
    let document = Document {
        meta: DocMeta {
            source_format: "hwp5".to_string(),
            source_version: file_header.version.to_string(),
        },
        metadata,
        header,
        sections,
        bin_streams,
        hwpx_settings_xml: None,
        hwpx_version_xml: None,
        hwpx_preview_image: None,
        hwp5_xml_template,
        hwp5_doc_history,
        // hwp5 출신 문서는 hwpx 패키지 잉여 엔트리가 없다.
        hwpx_extra_entries: Vec::new(),
        hwpx_bin_manifest: Vec::new(),
        hwpx_opf_extra_items: Vec::new(),
        hwpx_section_xmlns: Vec::new(),
    };
    Ok(ReadResult {
        document,
        warnings,
        // The bounded-read snapshot path does not decrypt ViewText (Task 2 adds
        // an explicit DistributionDoc refusal to BoundedReadSnapshot::open), so
        // a distribution document never reaches this construction site.
        unwrapped_distribution: false,
    })
}

/// HWP 5.0 파일을 IR로 읽는다. 야생 파일 대응을 위해 관용 모드로 스캔한다.
pub fn read_document(path: &Path) -> Result<ReadResult> {
    read_document_with_options(path, &ReadOptions::default())
}

/// Reads HWP5 with one transient set of options. An encrypted body is accepted
/// only for the evidence-backed EncryptVersion 4 profile and is handed to the
/// same record parser used by ordinary HWP5 documents.
pub fn read_document_with_options(path: &Path, options: &ReadOptions<'_>) -> Result<ReadResult> {
    let mut container = Hwp5Container::open(path)?;
    if container.file_header().is_encrypted() {
        return read_password_protected_document(&mut container, options);
    }
    read_document_from_container(&mut container)
}

fn read_document_from_container(container: &mut Hwp5Container) -> Result<ReadResult> {
    container.check_body_readable()?;

    let mut warnings = Vec::new();
    // Distribution documents (배포용문서) keep their body under `/ViewText/`
    // instead of `/BodyText/`; a near-empty `/BodyText/Section0` stub still
    // exists and still parses, but it is a decoy (GATE-01, PATTERNS.md
    // Pitfall 3) — the branch must be on the header bit, never stream presence.
    let is_distribution = container.file_header().is_distribution();
    let compressed = container.file_header().is_compressed();

    // DocInfo
    let doc_info_data = container.read_record_stream("/DocInfo")?;
    let scan = scan_stream(&doc_info_data, ScanMode::Tolerant)?;
    warnings.extend(scan.warnings.iter().map(|w| format!("[DocInfo] {w}")));
    let (header, doc_warnings) = parse_doc_info(&scan.roots);
    warnings.extend(doc_warnings.iter().map(|w| format!("[DocInfo] {w}")));

    // 본문 섹션들 (배포용 문서는 /ViewText/, 그 외엔 /BodyText/)
    let section_paths = if is_distribution {
        let paths = container.view_text_sections();
        if paths.is_empty() {
            return Err(Hwp5Error::StreamNotFound("/ViewText/Section0".to_string()));
        }
        paths
    } else {
        container.body_sections()
    };
    let mut sections = Vec::new();
    for stream_path in section_paths {
        let data = if is_distribution {
            let raw = container.read_stream_raw(&stream_path)?;
            crate::distdoc::decrypt_view_text_section(&raw, compressed)?
        } else {
            container.read_record_stream(&stream_path)?
        };
        let scan = scan_stream(&data, ScanMode::Tolerant)?;
        warnings.extend(scan.warnings.iter().map(|w| format!("[{stream_path}] {w}")));
        let (section, sec_warnings) = parse_section(&scan.roots);
        warnings.extend(sec_warnings.iter().map(|w| format!("[{stream_path}] {w}")));
        sections.push(section);
    }

    // 첨부 바이너리: 압축 플래그가 있으면 해제 시도, 실패 시 원본 사용
    // (BIN_DATA 레코드의 개별 압축 모드는 보수적으로 시도-폴백으로 흡수)
    let compressed = container.file_header().is_compressed();
    let mut bin_streams = Vec::new();
    for info in container.list_streams() {
        if let Some(name) = info.path.strip_prefix("/BinData/") {
            let raw = container.read_stream_raw(&info.path)?;
            let data = if compressed {
                crate::codec::decompress(&raw, &info.path).unwrap_or(raw)
            } else {
                raw
            };
            bin_streams.push(hwp_model::BinStream {
                name: name.to_string(),
                data,
            });
        }
    }

    // 문서 메타데이터 (요약 정보 — 최선 노력: 없거나 손상돼도 진단 계속)
    let metadata = container
        .read_stream_raw("/\u{5}HwpSummaryInformation")
        .ok()
        .map(|raw| crate::summary::parse_summary(&raw))
        .unwrap_or_default();

    // XMLTemplate·DocHistory 스토리지: 내용 해석 없이 원문 바이트 그대로 포착
    // (§3.2.10·§3.2.11·§4.4). IR 경유 되쓰기에서 writer가 재방출한다. 스토리지별
    // 압축 규칙이 스펙에 미기재라 **해제하지 않고** 바이트 그대로가 무손실이다.
    let mut hwp5_xml_template = Vec::new();
    let mut hwp5_doc_history = Vec::new();
    for info in container.list_streams() {
        if info.path.starts_with("/XMLTemplate/") {
            let raw = container.read_stream_raw(&info.path)?;
            hwp5_xml_template.push((info.path.clone(), raw));
        } else if info.path.starts_with("/DocHistory/") {
            let raw = container.read_stream_raw(&info.path)?;
            hwp5_doc_history.push((info.path.clone(), raw));
        }
    }

    let document = Document {
        meta: DocMeta {
            source_format: "hwp5".to_string(),
            source_version: container.file_header().version.to_string(),
        },
        metadata,
        header,
        sections,
        bin_streams,
        // hwp5 출신 문서는 hwpx 부속 파트가 없다 → None(쓰기 시 기본 상수).
        hwpx_settings_xml: None,
        hwpx_version_xml: None,
        hwpx_preview_image: None,
        hwp5_xml_template,
        hwp5_doc_history,
        // hwp5 출신 문서는 hwpx 패키지 잉여 엔트리가 없다.
        hwpx_extra_entries: Vec::new(),
        hwpx_bin_manifest: Vec::new(),
        hwpx_opf_extra_items: Vec::new(),
        hwpx_section_xmlns: Vec::new(),
    };
    Ok(ReadResult {
        document,
        warnings,
        unwrapped_distribution: is_distribution,
    })
}

fn is_evidenced_password_record_stream(path: &str) -> bool {
    path == "/DocInfo" || path == "/BodyText/Section0"
}

fn is_password_candidate_record_stream(path: &str) -> bool {
    path == "/DocInfo" || path.starts_with("/BodyText/") || path.starts_with("/ViewText/")
}

fn expected_password_record_tag(path: &str) -> Option<u16> {
    if path == "/DocInfo" {
        Some(crate::record::tag::DOCUMENT_PROPERTIES)
    } else if path == "/BodyText/Section0" {
        Some(crate::record::tag::PARA_HEADER)
    } else {
        None
    }
}

fn record_tree_contains_tag(nodes: &[crate::record::RecordNode], expected: u16) -> bool {
    nodes
        .iter()
        .any(|node| node.tag == expected || record_tree_contains_tag(&node.children, expected))
}

fn validate_password_record_identity(path: &str, data: &[u8]) -> Result<()> {
    let expected = expected_password_record_tag(path).ok_or(Hwp5Error::Encrypted)?;
    let scan = scan_stream(data, ScanMode::Strict).map_err(|_| Hwp5Error::Encrypted)?;
    if scan.record_count == 0 || !record_tree_contains_tag(&scan.roots, expected) {
        return Err(Hwp5Error::Encrypted);
    }
    Ok(())
}

fn checked_password_live_bytes(left: u64, right: u64) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
            resource: "password-protected HWP5 live buffers".to_string(),
            limit: password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
        })
}

fn read_password_protected_document(
    container: &mut Hwp5Container,
    options: &ReadOptions<'_>,
) -> Result<ReadResult> {
    let header = container.file_header().clone();
    header.check_body_readable_with_password(options.password)?;
    let password = options.password.ok_or(Hwp5Error::Encrypted)?;
    let stream_infos = container
        .list_streams_bounded(
            HWP5_PASSWORD_MAX_STREAMS,
            HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES,
        )
        .map_err(normalize_password_candidate_error)?;
    if stream_infos.iter().any(|stream| {
        is_password_candidate_record_stream(&stream.path)
            && !is_evidenced_password_record_stream(&stream.path)
    }) || !stream_infos.iter().any(|stream| stream.path == "/DocInfo")
        || !stream_infos
            .iter()
            .any(|stream| stream.path == "/BodyText/Section0")
    {
        return Err(Hwp5Error::UnsupportedPasswordProfile {
            encrypt_version: header.encrypt_version,
        });
    }

    let mut streams = BTreeMap::new();
    let mut retained_plaintext = 0u64;
    // Authenticate both observed record streams before reporting structural
    // limits from opaque streams. A candidate key can otherwise manufacture a
    // decompression-limit result that leaks a different public refusal.
    for stream in stream_infos
        .iter()
        .filter(|stream| is_evidenced_password_record_stream(&stream.path))
    {
        validate_live_bytes(stream.size, retained_plaintext)
            .map_err(normalize_password_candidate_error)?;
        let mut protected = zeroize::Zeroizing::new(container.read_stream_raw(&stream.path)?);
        if protected.len() as u64 != stream.size {
            return Err(Hwp5Error::Encrypted);
        }
        decrypt_hwp5_encrypt_version_4_in_place(&mut protected, password)?;
        let decrypted = if header.is_compressed() {
            let live_ciphertext = checked_password_live_bytes(retained_plaintext, stream.size)
                .map_err(normalize_password_candidate_error)?;
            validate_live_bytes(HWP5_PASSWORD_MAX_STREAM_BYTES, live_ciphertext)
                .map_err(normalize_password_candidate_error)?;
            let result = crate::codec::decompress_bounded(
                &protected,
                "password-protected HWP5 stream",
                HWP5_PASSWORD_MAX_STREAM_BYTES,
            );
            protected.zeroize();
            result.map_err(normalize_password_candidate_error)?
        } else {
            std::mem::take(&mut *protected)
        };
        protected.zeroize();

        let decrypted_size = decrypted.len() as u64;
        validate_live_bytes(decrypted_size, retained_plaintext)
            .map_err(normalize_password_candidate_error)?;
        // Strict record identity is the password validation boundary. Reserve
        // a second copy for the temporary strict scan before invoking it.
        let scan_live = checked_password_live_bytes(retained_plaintext, decrypted_size)
            .map_err(normalize_password_candidate_error)?;
        validate_live_bytes(decrypted_size, scan_live)
            .map_err(normalize_password_candidate_error)?;
        validate_password_record_identity(&stream.path, &decrypted)?;
        retained_plaintext = retained_plaintext
            .checked_add(decrypted_size)
            .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                resource: "password-protected HWP5 live buffers".to_string(),
                limit: password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
            })?;
        streams.insert(stream.path.clone(), decrypted);
    }

    // Every materialized stream, including opaque preservation paths, is
    // bounded before `read_stream_raw` can allocate it. These resource errors
    // are now authenticated structural failures, not credential outcomes.
    for stream in stream_infos
        .iter()
        .filter(|stream| !is_evidenced_password_record_stream(&stream.path))
    {
        validate_live_bytes(stream.size, retained_plaintext)?;
        let raw = container.read_stream_raw(&stream.path)?;
        if raw.len() as u64 != stream.size {
            return Err(Hwp5Error::Encrypted);
        }
        retained_plaintext = checked_password_live_bytes(retained_plaintext, stream.size)?;
        streams.insert(stream.path.clone(), raw);
    }

    // The regular parser owns record data while the decrypted stream map is
    // still live. Reserve that handoff before parsing, rather than after an
    // allocation has already occurred.
    validate_live_bytes(retained_plaintext, retained_plaintext)?;
    header.check_body_readable_after_password()?;
    read_document_from_streams(&header, &streams).map_err(|_| Hwp5Error::Encrypted)
}

fn normalize_password_candidate_error(_error: Hwp5Error) -> Hwp5Error {
    // The record identity proof has not completed yet, so an expansion limit
    // can be induced by a wrong candidate and must share the stable credential
    // refusal with every other failed candidate.
    Hwp5Error::Encrypted
}

#[cfg(test)]
mod bounded_tests {
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    use std::time::{SystemTime, UNIX_EPOCH};

    use aes::Aes128;
    use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
    use sha1::Digest as _;

    use super::*;

    fn temporary_hwp(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hwp5-bounded-{label}-{}-{nonce}.hwp",
            std::process::id()
        ))
    }

    fn limits() -> BoundedReadLimits {
        BoundedReadLimits {
            max_streams: 4_096,
            max_total_stream_name_bytes: 16 * 1_024 * 1_024,
            max_stream_bytes: 64 * 1_024,
            max_total_materialized_bytes: 4 * 1_024 * 1_024,
            max_records: 10_000,
            max_record_depth: 128,
        }
    }

    fn base_hwp(label: &str) -> std::path::PathBuf {
        let path = temporary_hwp(label);
        let document = hwp_convert::from_markdown("bounded");
        crate::write_document(&document, &path, &crate::WriteOptions::default()).unwrap();
        path
    }

    fn encrypt_hwp5_stream_for_test(plaintext: &[u8], password: &str) -> Vec<u8> {
        let password = password.as_bytes();
        let mut source = Vec::with_capacity(password.len() * 2);
        for (index, byte) in password.iter().copied().enumerate() {
            let previous = if index == 0 {
                0xec
            } else {
                password[index - 1]
            };
            source.push(previous.rotate_left(1));
            source.push(byte);
        }
        let digest = sha1::Sha1::digest(&source);
        let cipher = Aes128::new_from_slice(&digest[..16]).expect("fixed AES key length");
        let mut register = [0u8; 16];
        let mut encrypted = plaintext.to_vec();
        for block in encrypted.chunks_mut(16) {
            let mut original = [0u8; 16];
            original[..block.len()].copy_from_slice(block);
            let mut transformed = [0u8; 16];
            for bit_index in 0..128 {
                let byte_index = bit_index / 8;
                let bit_offset = bit_index % 8;
                let mut keystream = Block::<Aes128>::from(register);
                cipher.encrypt_block(&mut keystream);
                let input_bit = (original[byte_index] >> (7 - bit_offset)) & 1;
                let result_bit = input_bit ^ (keystream[0] >> 7);
                for index in 0..15 {
                    register[index] = (register[index] << 1) | (register[index + 1] >> 7);
                }
                register[15] = (register[15] << 1) | result_bit;
                transformed[byte_index] |= result_bit << (7 - bit_offset);
            }
            block.copy_from_slice(&transformed[..block.len()]);
        }
        encrypted
    }

    fn make_evidenced_password_hwp(path: &Path, password: &str, additional_attributes: u32) {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut cfb = cfb::CompoundFile::open(file).unwrap();
        let mut header = Vec::new();
        cfb.open_stream("/FileHeader")
            .unwrap()
            .read_to_end(&mut header)
            .unwrap();
        let attributes = u32::from_le_bytes(header[36..40].try_into().unwrap())
            | (1 << 1)
            | additional_attributes;
        header[36..40].copy_from_slice(&attributes.to_le_bytes());
        header[44..48].copy_from_slice(&4u32.to_le_bytes());
        let mut header_stream = cfb.open_stream("/FileHeader").unwrap();
        header_stream.set_len(0).unwrap();
        header_stream.seek(SeekFrom::Start(0)).unwrap();
        header_stream.write_all(&header).unwrap();
        drop(header_stream);

        for path in ["/DocInfo", "/BodyText/Section0"] {
            let mut raw = Vec::new();
            cfb.open_stream(path)
                .unwrap()
                .read_to_end(&mut raw)
                .unwrap();
            let encrypted = encrypt_hwp5_stream_for_test(&raw, password);
            let mut stream = cfb.open_stream(path).unwrap();
            stream.set_len(0).unwrap();
            stream.seek(SeekFrom::Start(0)).unwrap();
            stream.write_all(&encrypted).unwrap();
        }
        cfb.flush().unwrap();
    }

    #[test]
    fn evidenced_password_profile_reenters_the_normal_reader_with_exact_utf8() {
        let path = base_hwp("password-profile");
        let password = "\u{ac00}";
        make_evidenced_password_hwp(&path, password, 0);

        assert!(matches!(read_document(&path), Err(Hwp5Error::Encrypted)));
        let wrong = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some("\u{1100}\u{1161}"),
            },
        );
        assert!(matches!(wrong, Err(Hwp5Error::Encrypted)));
        let result = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        )
        .unwrap();
        assert_eq!(result.document.meta.source_format, "hwp5");
        assert_eq!(result.document.sections.len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wrong_password_precedes_certificate_refusal_and_success_resumes_gate_order() {
        const CERT_ENCRYPTED: u32 = 1 << 8;
        const DRM: u32 = 1 << 4;
        let path = base_hwp("password-gate-order");
        let password = "exact password";
        make_evidenced_password_hwp(&path, password, CERT_ENCRYPTED | DRM);

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some("wrong password"),
                },
            ),
            Err(Hwp5Error::Encrypted)
        ));
        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::CertEncrypted)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn protected_non_record_stream_is_limited_before_materialization() {
        let path = base_hwp("password-oversized-bindata");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            cfb.create_storage("/BinData").unwrap();
            let mut stream = cfb.create_new_stream("/BinData/oversized.bin").unwrap();
            stream.set_len(HWP5_PASSWORD_MAX_STREAM_BYTES + 1).unwrap();
            drop(stream);
            cfb.flush().unwrap();
        }

        let error = match read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        ) {
            Ok(_) => panic!("oversized opaque stream must be refused"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            Hwp5Error::ResourceLimitExceeded { resource, limit }
                if resource == "password-protected HWP5 stream"
                    && limit == HWP5_PASSWORD_MAX_STREAM_BYTES
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn compressed_candidate_limit_is_normalized_to_credential_refusal() {
        let path = base_hwp("password-candidate-limit");
        let candidate = "candidate-password";
        // Set the compressed header bit and make the candidate decrypt to a
        // valid raw-DEFLATE bomb. It reaches the resource guard before either
        // record identity proof, exactly where a wrong candidate must remain
        // indistinguishable from every other credential failure.
        make_evidenced_password_hwp(&path, candidate, 1);
        let compressed_bomb = crate::codec::compress(&vec![b'x'; 65 * 1024 * 1024]);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let encrypted = encrypt_hwp5_stream_for_test(&compressed_bomb, candidate);
            let mut stream = cfb.open_stream("/DocInfo").unwrap();
            stream.set_len(0).unwrap();
            stream.seek(SeekFrom::Start(0)).unwrap();
            stream.write_all(&encrypted).unwrap();
            drop(stream);
            cfb.flush().unwrap();
        }

        let error = match read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(candidate),
            },
        ) {
            Ok(_) => panic!("candidate bomb must not parse"),
            Err(error) => error,
        };
        assert!(matches!(error, Hwp5Error::Encrypted), "{error:?}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn compressed_script_bomb_is_rejected_before_semantic_parse() {
        let path = base_hwp("script-bomb");
        let bomb = crate::codec::compress(&vec![b'x'; 1024 * 1024]);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut stream = cfb.open_stream("/Scripts/DefaultJScript").unwrap();
            stream.set_len(0).unwrap();
            stream.seek(SeekFrom::Start(0)).unwrap();
            stream.write_all(&bomb).unwrap();
            drop(stream);
            cfb.flush().unwrap();
        }
        let error = BoundedReadSnapshot::open(&path, limits()).unwrap_err();
        assert!(
            matches!(
                &error,
                Hwp5Error::ResourceLimitExceeded { resource, limit: 65_536 }
                    if resource.contains("/Scripts/DefaultJScript")
            ),
            "{error:?}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn compressed_bindata_bomb_cannot_fall_back_to_raw_bytes() {
        let path = base_hwp("bindata-bomb");
        let bomb = crate::codec::compress(&vec![b'y'; 1024 * 1024]);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            cfb.create_storage("/BinData").unwrap();
            cfb.create_new_stream("/BinData/BIN0001.bin")
                .unwrap()
                .write_all(&bomb)
                .unwrap();
            cfb.flush().unwrap();
        }
        let error = BoundedReadSnapshot::open(&path, limits()).unwrap_err();
        assert!(matches!(
            error,
            Hwp5Error::ResourceLimitExceeded { resource, limit: 65_536 }
                if resource.contains("/BinData/BIN0001.bin")
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn thousands_of_empty_streams_are_rejected_during_directory_walk() {
        let path = base_hwp("stream-count");
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            cfb.create_storage("/Empty").unwrap();
            for index in 0..4_096 {
                cfb.create_new_stream(format!("/Empty/{index:04}")).unwrap();
            }
            cfb.flush().unwrap();
        }
        let error = BoundedReadSnapshot::open(&path, limits()).unwrap_err();
        assert!(matches!(
            error,
            Hwp5Error::StructureLimitExceeded { resource, limit: 4_096 }
                if resource == "CFB stream count"
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_stub_is_not_a_macro_but_real_script_is_detected() {
        let fixture = |name: &str| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/hwp5")
                .join(name)
        };
        let generous = BoundedReadLimits {
            max_streams: 4_096,
            max_total_stream_name_bytes: 16 * 1_024 * 1_024,
            max_stream_bytes: 64 * 1_024 * 1_024,
            max_total_materialized_bytes: 128 * 1_024 * 1_024,
            max_records: 200_000,
            max_record_depth: 128,
        };
        // fixtures/hwp5/*.hwp 는 gitignore(로컬 전용)라 CI에는 없다. 없으면 skip한다.
        if !fixture("hello_world.hwp").exists() || !fixture("annual_report.hwp").exists() {
            eprintln!("skip: fixtures/hwp5 부재");
            return;
        }
        let empty = BoundedReadSnapshot::open(&fixture("hello_world.hwp"), generous).unwrap();
        assert_eq!(empty.script_presence(), ScriptPresence::Absent);
        let real = BoundedReadSnapshot::open(&fixture("annual_report.hwp"), generous).unwrap();
        assert_eq!(real.script_presence(), ScriptPresence::Present);
    }

    #[test]
    fn opaque_script_payload_is_indeterminate() {
        assert_eq!(
            parse_script_source(&[1, 0, 0, 0, b'a']),
            ScriptPresence::Indeterminate
        );
    }
}
