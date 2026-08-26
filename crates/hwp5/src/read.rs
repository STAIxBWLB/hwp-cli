//! 최상위: HWP 5.0 파일 → [`Document`].

use std::collections::BTreeMap;
use std::mem::size_of;
use std::path::Path;

use hwp_model::{DocMeta, Document};
use zeroize::Zeroize as _;

use crate::body_text::parse_section;
use crate::container::{Hwp5Container, StreamInfo, is_record_stream};
use crate::doc_info::parse_doc_info;
use crate::error::{Hwp5Error, Result};
use crate::file_header::FileHeader;
use crate::record::{
    RecordHeader, RecordScanBudget, RecordScanLimits, ScanMode, scan_stream, scan_stream_bounded,
    walk_stream_strict,
};

mod password;

pub(crate) const PASSWORD_PROTECTED_DOCUMENT_LIVE_LIMIT: u64 =
    password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES;
use password::{
    HWP5_PASSWORD_MAX_STREAM_BYTES, HWP5_PASSWORD_MAX_STREAMS,
    HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES, decrypt_hwp5_encrypt_version_4_in_place,
    validate_live_bytes, validate_transform_bytes,
};

const HWP5_PASSWORD_MAX_RECORDS: usize = 131_072;
const HWP5_PASSWORD_MAX_RECORD_DEPTH: usize = 128;

// The protected reader is built with Rust 1.93.  Its Vec/String amortized
// growth never keeps more than twice the requested element capacity live.
// Reserve that documented implementation bound explicitly at every owner;
// never hide it in a semantic "multiplier" over source bytes.
const PROTECTED_VEC_GROWTH_BOUND: u64 = 2;
const PROTECTED_UTF8_BYTES_PER_UTF16_UNIT: u64 = 3;
const PROTECTED_OPAQUE_COPIES_PER_RECORD: u64 = 3;

#[derive(Debug, Default)]
struct PasswordSemanticReservation {
    scanner_bytes: u64,
    semantic_bytes: u64,
    summary_bytes: u64,
}

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
    read_document_from_streams_with_budget(file_header, streams, None)
}

struct ParsedDocument {
    warnings: Vec<String>,
    metadata: hwp_model::Metadata,
    header: hwp_model::DocHeader,
    sections: Vec<hwp_model::Section>,
}

fn parse_document_streams(
    file_header: &FileHeader,
    streams: &BTreeMap<String, Vec<u8>>,
    mut record_budget: Option<&mut RecordScanBudget>,
) -> Result<ParsedDocument> {
    let mut warnings = Vec::new();
    let doc_info_data = streams
        .get("/DocInfo")
        .ok_or_else(|| Hwp5Error::StreamNotFound("/DocInfo".to_string()))?;
    let scan = match record_budget.as_deref_mut() {
        Some(budget) => scan_stream_bounded(doc_info_data, ScanMode::Tolerant, budget)?,
        None => scan_stream(doc_info_data, ScanMode::Tolerant)?,
    };
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

    let section_prefix = if file_header.is_distribution() {
        "/ViewText/Section"
    } else {
        "/BodyText/Section"
    };
    let mut body_sections: Vec<&str> = streams
        .keys()
        .filter(|path| path.starts_with(section_prefix))
        .map(String::as_str)
        .collect();
    body_sections.sort_by_key(|path| {
        path.trim_start_matches(section_prefix)
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let mut sections = Vec::with_capacity(body_sections.len());
    for stream_path in body_sections {
        let data = &streams[stream_path];
        let scan = match record_budget.as_deref_mut() {
            Some(budget) => scan_stream_bounded(data, ScanMode::Tolerant, budget)?,
            None => scan_stream(data, ScanMode::Tolerant)?,
        };
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

    let metadata = streams
        .get("/\u{5}HwpSummaryInformation")
        .map(|raw| crate::summary::parse_summary(raw))
        .unwrap_or_default();
    Ok(ParsedDocument {
        warnings,
        metadata,
        header,
        sections,
    })
}

fn read_result_from_parsed(
    file_header: &FileHeader,
    parsed: ParsedDocument,
    bin_streams: Vec<hwp_model::BinStream>,
    hwp5_xml_template: Vec<(String, Vec<u8>)>,
    hwp5_doc_history: Vec<(String, Vec<u8>)>,
) -> ReadResult {
    let document = Document {
        meta: DocMeta {
            source_format: "hwp5".to_string(),
            source_version: file_header.version.to_string(),
        },
        metadata: parsed.metadata,
        header: parsed.header,
        sections: parsed.sections,
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
    ReadResult {
        document,
        warnings: parsed.warnings,
        // BoundedReadSnapshot refuses distribution input before this point;
        // the password-aware path explicitly unwraps ViewText first and does
        // reach this shared construction site.
        unwrapped_distribution: file_header.is_distribution(),
    }
}

/// Parses a materialized stream map. Password-protected reads provide a
/// bounded scanner budget so record trees cannot exceed their authenticated
/// live-buffer reservation; ordinary reads retain the legacy tolerant path.
fn read_document_from_streams_with_budget(
    file_header: &FileHeader,
    streams: &BTreeMap<String, Vec<u8>>,
    record_budget: Option<&mut RecordScanBudget>,
) -> Result<ReadResult> {
    let parsed = parse_document_streams(file_header, streams, record_budget)?;
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
    Ok(read_result_from_parsed(
        file_header,
        parsed,
        bin_streams,
        hwp5_xml_template,
        hwp5_doc_history,
    ))
}

/// Transfers opaque source streams into the final HWP5 Document. The ordinary
/// reader deliberately keeps its clone-based behavior above, but a protected
/// read must not retain the materialized source map *and* an equal preservation
/// copy while its authenticated parser trees are still live.
fn take_preserved_streams(
    streams: &mut BTreeMap<String, Vec<u8>>,
    prefix: &str,
) -> Vec<(String, Vec<u8>)> {
    let paths: Vec<String> = streams
        .keys()
        .filter(|path| path.starts_with(prefix))
        .cloned()
        .collect();
    paths
        .into_iter()
        .filter_map(|path| streams.remove(&path).map(|data| (path, data)))
        .collect()
}

fn read_password_document_from_owned_streams(
    file_header: &FileHeader,
    mut streams: BTreeMap<String, Vec<u8>>,
    record_budget: &mut RecordScanBudget,
) -> Result<ReadResult> {
    let parsed = parse_document_streams(file_header, &streams, Some(record_budget))?;
    // Metadata is parsed into owned strings, so its source stream is no longer
    // needed after semantic parsing. Drop it before moving the opaque owners.
    // This keeps the pre-reserved metadata/string envelope transient.
    let _ = streams.remove("/\u{5}HwpSummaryInformation");
    let bin_streams = take_preserved_streams(&mut streams, "/BinData/")
        .into_iter()
        .filter_map(|(path, data)| {
            path.strip_prefix("/BinData/")
                .map(|name| hwp_model::BinStream {
                    name: name.to_string(),
                    data,
                })
        })
        .collect();
    let hwp5_xml_template = take_preserved_streams(&mut streams, "/XMLTemplate/");
    let hwp5_doc_history = take_preserved_streams(&mut streams, "/DocHistory/");
    Ok(read_result_from_parsed(
        file_header,
        parsed,
        bin_streams,
        hwp5_xml_template,
        hwp5_doc_history,
    ))
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
    path == "/DocInfo"
        || path.starts_with("/BodyText/Section")
        || path.starts_with("/ViewText/Section")
}

/// The protected path materializes only bytes that the final HWP5 Document
/// actually owns. FileHeader and unrelated opaque streams have no IR owner,
/// so retaining them would spend the protected live-buffer budget without any
/// source-preservation benefit.
fn is_password_document_stream(path: &str) -> bool {
    is_evidenced_password_record_stream(path)
        || path == "/\u{5}HwpSummaryInformation"
        || path.starts_with("/BinData/")
        || path.starts_with("/XMLTemplate/")
        || path.starts_with("/DocHistory/")
}

fn is_password_candidate_record_stream(path: &str) -> bool {
    path == "/DocInfo" || path.starts_with("/BodyText/") || path.starts_with("/ViewText/")
}

fn expected_password_record_tag(path: &str) -> Option<u16> {
    if path == "/DocInfo" {
        Some(crate::record::tag::DOCUMENT_PROPERTIES)
    } else if path.starts_with("/BodyText/Section") || path.starts_with("/ViewText/Section") {
        Some(crate::record::tag::PARA_HEADER)
    } else {
        None
    }
}

fn validate_password_record_identity(path: &str, data: &[u8]) -> Result<()> {
    let expected = expected_password_record_tag(path).ok_or(Hwp5Error::Encrypted)?;
    let found_expected_tag = walk_stream_strict(
        data,
        expected,
        HWP5_PASSWORD_MAX_RECORDS,
        HWP5_PASSWORD_MAX_RECORD_DEPTH,
    )
    .map_err(|_| Hwp5Error::Encrypted)?;
    if !found_expected_tag {
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

fn password_live_limit_error() -> Hwp5Error {
    Hwp5Error::ResourceLimitExceeded {
        resource: "password-protected HWP5 live buffers".to_string(),
        limit: password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
    }
}

fn checked_password_reservation(total: &mut u64, amount: u64) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or_else(password_live_limit_error)?;
    if *total > password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES {
        return Err(password_live_limit_error());
    }
    Ok(())
}

fn checked_password_product(left: u64, right: u64) -> Result<u64> {
    left.checked_mul(right)
        .ok_or_else(password_live_limit_error)
}

fn reserve_growing_owner(total: &mut u64, elements: u64, element_size: usize) -> Result<()> {
    let bytes = checked_password_product(elements, element_size as u64)?;
    checked_password_reservation(
        total,
        checked_password_product(bytes, PROTECTED_VEC_GROWTH_BOUND)?,
    )
}

/// Accounts for every normalized path allocation that can coexist in the
/// protected reader. The directory listing owns all names while it is drained;
/// the materialized map then owns the document-path keys; BinData additionally
/// owns its stripped final `BinStream::name` while its map key is still live.
///
/// This deliberately charges the fixed owners as well as their UTF-8 bytes so
/// a high-cardinality CFB directory cannot hide behind short individual names.
fn password_path_reservation(streams: &[StreamInfo]) -> Result<u64> {
    let mut total = 0u64;
    for stream in streams {
        let path_bytes = stream.path.len() as u64;
        // `Vec<StreamInfo>` retains each entry while paths are drained. Its
        // capacity can be up to twice the observed element count.
        checked_password_reservation(&mut total, path_bytes)?;
        reserve_growing_owner(&mut total, 1, size_of::<StreamInfo>())?;

        if is_password_document_stream(&stream.path) {
            // BTreeMap key plus value slot. The implementation's internal leaf
            // link/length fields are bounded by three machine words per entry.
            checked_password_reservation(&mut total, path_bytes)?;
            reserve_growing_owner(&mut total, 1, size_of::<(String, Vec<u8>)>())?;
            reserve_growing_owner(&mut total, 3, size_of::<usize>())?;
        }
        if let Some(name) = stream.path.strip_prefix("/BinData/") {
            // `BinStream` is built before the source map is dropped. Its short
            // name is a distinct String, unlike XMLTemplate/DocHistory paths
            // which are moved into their final owners.
            checked_password_reservation(&mut total, name.len() as u64)?;
            reserve_growing_owner(&mut total, 1, size_of::<hwp_model::BinStream>())?;
        }
    }
    Ok(total)
}

fn reserve_record_semantics(total: &mut u64, header: RecordHeader) -> Result<()> {
    let payload = u64::from(header.size);

    // `to_opaque` is recursive and the control parser can simultaneously keep
    // `extras`, `raw_children`, and one specialized raw slot. Charge all three
    // actual raw payload owners plus their OpaqueRecord/Control containers.
    for _ in 0..PROTECTED_OPAQUE_COPIES_PER_RECORD {
        checked_password_reservation(total, payload)?;
        reserve_growing_owner(total, 1, size_of::<hwp_model::OpaqueRecord>())?;
    }
    reserve_growing_owner(total, 1, size_of::<hwp_model::Control>())?;
    reserve_growing_owner(total, 1, size_of::<hwp_model::Paragraph>())?;
    reserve_growing_owner(total, 1, size_of::<hwp_model::Section>())?;

    match header.tag {
        crate::record::tag::PARA_TEXT => {
            // `decode_para_text` emits at most one HwpChar per UTF-16 unit.
            // Inline/extended controls additionally allocate their 12-byte
            // payload for every eight input units.
            let units = payload.div_ceil(2);
            reserve_growing_owner(total, units, size_of::<hwp_model::HwpChar>())?;
            checked_password_reservation(total, checked_password_product(units / 8, 12)?)?;
        }
        crate::record::tag::PARA_CHAR_SHAPE => {
            reserve_growing_owner(
                total,
                payload / 8,
                size_of::<(u32, hwp_model::CharShapeId)>(),
            )?;
        }
        crate::record::tag::PARA_LINE_SEG => {
            reserve_growing_owner(total, payload / 36, size_of::<hwp_model::LineSeg>())?;
        }
        _ => {
            // The typed DocInfo/control parsers only decode record-local HWP
            // strings. Treat every non-text record as a possible typed string
            // owner: source UTF-16 units can expand to three UTF-8 bytes, and
            // String's growth can retain a second allocation while decoding.
            let units = payload / 2;
            let utf8 = checked_password_product(units, PROTECTED_UTF8_BYTES_PER_UTF16_UNIT)?;
            checked_password_reservation(
                total,
                checked_password_product(utf8, PROTECTED_VEC_GROWTH_BOUND)?,
            )?;
        }
    }
    Ok(())
}

fn reserve_record_scanner(total: &mut u64, header: RecordHeader) -> Result<()> {
    let payload = u64::from(header.size);
    checked_password_reservation(total, payload)?;
    // `scan_stream_with_budget` keeps the flat `(RecordHeader, Vec<u8>)`,
    // RecordNode tree, parent/root vectors, and tolerant warning owner alive
    // during forest construction. These are concrete Rust owner types rather
    // than a payload multiplier.
    let owner_bytes = (size_of::<(RecordHeader, Vec<u8>)>() as u64)
        .checked_add(checked_password_product(
            2,
            size_of::<crate::record::RecordNode>() as u64,
        )?)
        .and_then(|bytes| bytes.checked_add(size_of::<String>() as u64))
        .ok_or_else(password_live_limit_error)?;
    // The scanner's existing 512-byte per-record contract also covers the
    // bounded Korean diagnostic strings emitted by tolerant parsing. Keep it
    // as a floor, but derive all owner storage above from concrete types.
    checked_password_reservation(
        total,
        checked_password_product(owner_bytes.max(512), PROTECTED_VEC_GROWTH_BOUND)?,
    )?;
    Ok(())
}

fn reserve_summary_metadata(data: &[u8]) -> Result<u64> {
    const SUMMARY_PIDS: [u32; 6] = [2, 3, 4, 5, 6, 8];
    const VT_LPWSTR: u32 = 31;
    let mut final_strings = 0u64;
    let mut largest_utf16_scratch = 0u64;
    let mut option_owners = 0u64;

    let Some(0xfffe) = data
        .get(0..2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    else {
        return Ok(0);
    };
    let Some(section_count) = data
        .get(24..28)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    else {
        return Ok(0);
    };
    if section_count == 0 {
        return Ok(0);
    }
    let Some(section_offset) = data
        .get(44..48)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize)
    else {
        return Ok(0);
    };
    let Some(property_count) = data
        .get(section_offset.saturating_add(4)..section_offset.saturating_add(8))
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize)
    else {
        return Ok(0);
    };
    let table = match section_offset.checked_add(8) {
        Some(value) => value,
        None => return Ok(0),
    };
    for index in 0..property_count {
        let Some(entry) = index
            .checked_mul(8)
            .and_then(|offset| table.checked_add(offset))
        else {
            break;
        };
        let Some(pid) = data
            .get(entry..entry.saturating_add(4))
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
        else {
            break;
        };
        if !SUMMARY_PIDS.contains(&pid) {
            continue;
        }
        let Some(value_offset) = data
            .get(entry.saturating_add(4)..entry.saturating_add(8))
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize)
        else {
            break;
        };
        let Some(value) = section_offset.checked_add(value_offset) else {
            continue;
        };
        let Some(kind) = data
            .get(value..value.saturating_add(4))
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
        else {
            continue;
        };
        if kind != VT_LPWSTR {
            continue;
        }
        let Some(count) = data
            .get(value.saturating_add(4)..value.saturating_add(8))
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize)
        else {
            continue;
        };
        let Some(chars) = value.checked_add(8) else {
            continue;
        };
        let available = data.len().saturating_sub(chars) / 2;
        let units = count.min(available);
        let units = (0..units)
            .find(|index| {
                let offset = chars + index * 2;
                data.get(offset..offset + 2)
                    .is_some_and(|bytes| bytes == [0, 0])
            })
            .unwrap_or(units);
        if units == 0 {
            continue;
        }
        let units = units as u64;
        let utf8 = checked_password_product(units, PROTECTED_UTF8_BYTES_PER_UTF16_UNIT)?;
        final_strings = final_strings
            .checked_add(checked_password_product(utf8, PROTECTED_VEC_GROWTH_BOUND)?)
            .ok_or_else(password_live_limit_error)?;
        largest_utf16_scratch = largest_utf16_scratch.max(checked_password_product(units, 2)?);
        option_owners = option_owners
            .checked_add(size_of::<Option<String>>() as u64)
            .ok_or_else(password_live_limit_error)?;
    }
    let mut total = 0;
    checked_password_reservation(&mut total, final_strings)?;
    checked_password_reservation(&mut total, largest_utf16_scratch)?;
    checked_password_reservation(&mut total, option_owners)?;
    Ok(total)
}

/// Performs the authenticated, no-allocation preflight before record scanning.
/// The result reserves exact observed record counts/tags/payload sizes, plus
/// the six metadata PIDs' real alias multiplicity, before any semantic owner
/// can be allocated.
fn password_semantic_reservation(
    file_header: &FileHeader,
    streams: &BTreeMap<String, Vec<u8>>,
) -> Result<PasswordSemanticReservation> {
    let mut reservation = PasswordSemanticReservation::default();
    let section_prefix = if file_header.is_distribution() {
        "/ViewText/Section"
    } else {
        "/BodyText/Section"
    };
    for (_, data) in streams
        .iter()
        .filter(|(path, _)| path.as_str() == "/DocInfo" || path.starts_with(section_prefix))
    {
        let mut reader = crate::codec::ByteReader::new(data);
        while !reader.is_empty() {
            let header = RecordHeader::decode(&mut reader)?;
            reader.read_bytes(header.size as usize)?;
            reserve_record_scanner(&mut reservation.scanner_bytes, header)?;
            reserve_record_semantics(&mut reservation.semantic_bytes, header)?;
        }
    }
    if let Some(summary) = streams.get("/\u{5}HwpSummaryInformation") {
        reservation.summary_bytes = reserve_summary_metadata(summary)?;
    }
    Ok(reservation)
}

fn password_record_scan_budget(
    retained_plaintext: u64,
    path_bytes: u64,
    semantic: PasswordSemanticReservation,
) -> Result<RecordScanBudget> {
    let mut total = 0;
    checked_password_reservation(&mut total, retained_plaintext)?;
    checked_password_reservation(&mut total, path_bytes)?;
    checked_password_reservation(&mut total, semantic.scanner_bytes)?;
    checked_password_reservation(&mut total, semantic.semantic_bytes)?;
    checked_password_reservation(&mut total, semantic.summary_bytes)?;
    Ok(RecordScanBudget::new(RecordScanLimits {
        max_records: HWP5_PASSWORD_MAX_RECORDS,
        max_depth: HWP5_PASSWORD_MAX_RECORD_DEPTH,
        max_allocation_bytes: semantic.scanner_bytes,
    }))
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
    let required_section = if header.is_distribution() {
        "/ViewText/Section0"
    } else {
        "/BodyText/Section0"
    };
    if stream_infos.iter().any(|stream| {
        is_password_candidate_record_stream(&stream.path)
            && !is_evidenced_password_record_stream(&stream.path)
    }) || !stream_infos.iter().any(|stream| stream.path == "/DocInfo")
        || !stream_infos
            .iter()
            .any(|stream| stream.path == required_section)
    {
        return Err(Hwp5Error::UnsupportedPasswordProfile {
            encrypt_version: header.encrypt_version,
        });
    }
    // Charge every path owner before draining the directory listing. This
    // covers the simultaneous listing/map/final-owner peak, even though the
    // list itself is dropped before semantic parsing.
    let path_bytes = password_path_reservation(&stream_infos)?;
    let (record_infos, document_infos): (Vec<_>, Vec<_>) = stream_infos
        .into_iter()
        .filter(|stream| is_password_document_stream(&stream.path))
        .partition(|stream| is_evidenced_password_record_stream(&stream.path));
    let transform_bytes = record_infos.iter().try_fold(0u64, |total, stream| {
        total.checked_add(stream.size).ok_or(Hwp5Error::Encrypted)
    })?;
    validate_transform_bytes(transform_bytes).map_err(normalize_password_candidate_error)?;

    let mut streams = BTreeMap::new();
    let mut retained_plaintext = 0u64;
    // Authenticate both observed record streams before reporting structural
    // limits from opaque streams. A candidate key can otherwise manufacture a
    // decompression-limit result that leaks a different public refusal.
    for stream in record_infos {
        let retained_with_paths = checked_password_live_bytes(retained_plaintext, path_bytes)
            .map_err(normalize_password_candidate_error)?;
        validate_live_bytes(stream.size, retained_with_paths)
            .map_err(normalize_password_candidate_error)?;
        let is_view_text = stream.path.starts_with("/ViewText/Section");
        let decrypted = if is_view_text {
            let mut protected = zeroize::Zeroizing::new(container.read_stream_raw(&stream.path)?);
            if protected.len() as u64 != stream.size {
                return Err(Hwp5Error::Encrypted);
            }
            decrypt_hwp5_encrypt_version_4_in_place(&mut protected, password)?;
            let doubled_ciphertext = stream.size.checked_mul(2).ok_or(Hwp5Error::Encrypted)?;
            let live_intermediates =
                checked_password_live_bytes(retained_with_paths, doubled_ciphertext)
                    .map_err(normalize_password_candidate_error)?;
            validate_live_bytes(HWP5_PASSWORD_MAX_STREAM_BYTES, live_intermediates)
                .map_err(normalize_password_candidate_error)?;
            let result = crate::distdoc::decrypt_view_text_section_bounded(
                &protected,
                header.is_compressed(),
                HWP5_PASSWORD_MAX_STREAM_BYTES,
            );
            protected.zeroize();
            result.map_err(normalize_password_candidate_error)?
        } else if header.is_compressed() {
            // Keep ciphertext live only while raw-DEFLATE is expanded. Its
            // buffer is zeroized and dropped before identity walking or tree
            // construction, and the expansion reservation includes it.
            let mut protected = zeroize::Zeroizing::new(container.read_stream_raw(&stream.path)?);
            if protected.len() as u64 != stream.size {
                return Err(Hwp5Error::Encrypted);
            }
            decrypt_hwp5_encrypt_version_4_in_place(&mut protected, password)?;
            let live_ciphertext = checked_password_live_bytes(retained_with_paths, stream.size)
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
            let mut protected = zeroize::Zeroizing::new(container.read_stream_raw(&stream.path)?);
            if protected.len() as u64 != stream.size {
                return Err(Hwp5Error::Encrypted);
            }
            decrypt_hwp5_encrypt_version_4_in_place(&mut protected, password)?;
            std::mem::take(&mut *protected)
        };

        let decrypted_size = decrypted.len() as u64;
        validate_live_bytes(decrypted_size, retained_with_paths)
            .map_err(normalize_password_candidate_error)?;
        // Strict identity validation is a no-allocation header walk. It is the
        // credential boundary before any RecordNode or payload clone exists.
        validate_password_record_identity(&stream.path, &decrypted)?;
        retained_plaintext = retained_plaintext
            .checked_add(decrypted_size)
            .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                resource: "password-protected HWP5 live buffers".to_string(),
                limit: password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
            })?;
        streams.insert(stream.path, decrypted);
    }

    // Every materialized stream, including opaque preservation paths, is
    // bounded before `read_stream_raw` can allocate it. These resource errors
    // are now authenticated structural failures, not credential outcomes.
    for stream in document_infos {
        let retained_with_paths = checked_password_live_bytes(retained_plaintext, path_bytes)?;
        validate_live_bytes(stream.size, retained_with_paths)?;
        let raw = container.read_stream_raw(&stream.path)?;
        if raw.len() as u64 != stream.size {
            return Err(Hwp5Error::Encrypted);
        }
        let data = if header.is_compressed() && stream.path.starts_with("/BinData/") {
            // Match the ordinary reader's try-DEFLATE contract for BinData,
            // but reserve the simultaneous raw and expanded owners before
            // decompression. Malformed data may fall back to its bounded raw
            // bytes; an expansion-limit failure remains fatal.
            let live_raw = checked_password_live_bytes(retained_with_paths, stream.size)?;
            validate_live_bytes(HWP5_PASSWORD_MAX_STREAM_BYTES, live_raw)?;
            match crate::codec::decompress_bounded(
                &raw,
                &stream.path,
                HWP5_PASSWORD_MAX_STREAM_BYTES,
            ) {
                Ok(data) => data,
                Err(Hwp5Error::Decompress { .. }) => raw,
                Err(error) => return Err(error),
            }
        } else {
            raw
        };
        let materialized_size = data.len() as u64;
        validate_live_bytes(materialized_size, retained_with_paths)?;
        retained_plaintext = checked_password_live_bytes(retained_plaintext, materialized_size)?;
        streams.insert(stream.path, data);
    }

    // The regular parser owns record data while the decrypted stream map is
    // still live. The authenticated header-only preflight derives every
    // semantic reservation from the observed records and Summary aliases
    // before `scan_stream_bounded` can allocate a tree or IR owner.
    let semantic = password_semantic_reservation(&header, &streams)
        .map_err(normalize_password_candidate_error)?;
    let mut record_budget = password_record_scan_budget(retained_plaintext, path_bytes, semantic)?;
    header.check_body_readable_after_password()?;
    read_password_document_from_owned_streams(&header, streams, &mut record_budget)
        .map_err(|_| Hwp5Error::Encrypted)
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

        let protected_paths: Vec<String> = cfb
            .walk()
            .filter(|entry| entry.is_stream())
            .map(|entry| entry.path().to_string_lossy().replace('\\', "/"))
            .filter(|path| is_evidenced_password_record_stream(path))
            .collect();
        for path in protected_paths {
            let mut raw = Vec::new();
            cfb.open_stream(&path)
                .unwrap()
                .read_to_end(&mut raw)
                .unwrap();
            if additional_attributes & 1 != 0 && !path.starts_with("/ViewText/") {
                raw = crate::codec::compress(&raw);
            }
            let encrypted = encrypt_hwp5_stream_for_test(&raw, password);
            let mut stream = cfb.open_stream(&path).unwrap();
            stream.set_len(0).unwrap();
            stream.seek(SeekFrom::Start(0)).unwrap();
            stream.write_all(&encrypted).unwrap();
        }
        cfb.flush().unwrap();
    }

    fn replace_encrypted_stream_for_test(
        path: &Path,
        stream_path: &str,
        protected_bytes: &[u8],
        password: &str,
    ) {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut cfb = cfb::CompoundFile::open(file).unwrap();
        let encrypted = encrypt_hwp5_stream_for_test(protected_bytes, password);
        let mut stream = cfb.open_stream(stream_path).unwrap();
        stream.set_len(0).unwrap();
        stream.seek(SeekFrom::Start(0)).unwrap();
        stream.write_all(&encrypted).unwrap();
        drop(stream);
        cfb.flush().unwrap();
    }

    fn repeated_empty_records(tag: u16, count: usize) -> Vec<u8> {
        u32::from(tag).to_le_bytes().repeat(count)
    }

    fn encoded_record(tag: u16, level: u16, payload: Vec<u8>) -> Vec<u8> {
        let mut writer = crate::codec::ByteWriter::new();
        RecordHeader {
            tag,
            level,
            size: payload.len() as u32,
        }
        .encode(&mut writer);
        writer.write_bytes(&payload);
        writer.into_bytes()
    }

    fn para_text_record_stream(payload_bytes: usize) -> Vec<u8> {
        let mut stream = encoded_record(crate::record::tag::PARA_HEADER, 0, vec![0; 22]);
        let mut text = Vec::with_capacity(payload_bytes);
        for _ in 0..payload_bytes / 2 {
            text.extend_from_slice(&0xac00u16.to_le_bytes());
        }
        stream.extend(encoded_record(crate::record::tag::PARA_TEXT, 1, text));
        stream
    }

    fn aliased_summary_information(payload_bytes: usize) -> Vec<u8> {
        const SECTION_OFFSET: usize = 48;
        const TABLE_OFFSET: usize = SECTION_OFFSET + 8;
        const VALUE_OFFSET: usize = TABLE_OFFSET + 6 * 8;
        const PIDS: [u32; 6] = [2, 3, 4, 5, 6, 8];
        let units = (payload_bytes.saturating_sub(VALUE_OFFSET + 10)) / 2;
        let mut data = vec![0; VALUE_OFFSET + 8 + (units + 1) * 2];
        data[0..2].copy_from_slice(&0xfffeu16.to_le_bytes());
        data[24..28].copy_from_slice(&1u32.to_le_bytes());
        data[44..48].copy_from_slice(&(SECTION_OFFSET as u32).to_le_bytes());
        data[SECTION_OFFSET + 4..SECTION_OFFSET + 8]
            .copy_from_slice(&(PIDS.len() as u32).to_le_bytes());
        for (index, pid) in PIDS.into_iter().enumerate() {
            let entry = TABLE_OFFSET + index * 8;
            data[entry..entry + 4].copy_from_slice(&pid.to_le_bytes());
            data[entry + 4..entry + 8]
                .copy_from_slice(&((VALUE_OFFSET - SECTION_OFFSET) as u32).to_le_bytes());
        }
        data[VALUE_OFFSET..VALUE_OFFSET + 4].copy_from_slice(&31u32.to_le_bytes());
        data[VALUE_OFFSET + 4..VALUE_OFFSET + 8]
            .copy_from_slice(&((units + 1) as u32).to_le_bytes());
        for offset in (VALUE_OFFSET + 8..VALUE_OFFSET + 8 + units * 2).step_by(2) {
            data[offset..offset + 2].copy_from_slice(&0xac00u16.to_le_bytes());
        }
        data
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
    fn protected_reader_authenticates_and_parses_every_body_section() {
        let path = base_hwp("password-multiple-sections");
        let password = "correct-password";
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut section = Vec::new();
            cfb.open_stream("/BodyText/Section0")
                .unwrap()
                .read_to_end(&mut section)
                .unwrap();
            cfb.create_new_stream("/BodyText/Section1")
                .unwrap()
                .write_all(&section)
                .unwrap();
            cfb.flush().unwrap();
        }
        make_evidenced_password_hwp(&path, password, 0);

        let result = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        )
        .unwrap();
        assert_eq!(result.document.sections.len(), 2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn protected_distribution_reader_unwraps_every_view_text_section() {
        const DISTRIBUTION: u32 = 1 << 2;
        let path = base_hwp("password-distribution-sections");
        let password = "correct-password";
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut compressed_section = Vec::new();
            cfb.open_stream("/BodyText/Section0")
                .unwrap()
                .read_to_end(&mut compressed_section)
                .unwrap();
            let section = crate::codec::decompress(&compressed_section, "test section").unwrap();
            cfb.create_storage("/ViewText").unwrap();
            for index in 0..2 {
                let protected = crate::distdoc::encrypt_view_text_section_for_test(&section, true);
                cfb.create_new_stream(format!("/ViewText/Section{index}"))
                    .unwrap()
                    .write_all(&protected)
                    .unwrap();
            }
            cfb.flush().unwrap();
        }
        make_evidenced_password_hwp(&path, password, DISTRIBUTION);

        let result = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        )
        .unwrap();
        assert!(result.unwrapped_distribution);
        assert_eq!(result.document.sections.len(), 2);
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
    fn protected_compressed_bindata_is_materialized_like_an_ordinary_read() {
        let path = base_hwp("password-compressed-bindata");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);
        let expected = b"synthetic image payload".repeat(64);
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
                .write_all(&crate::codec::compress(&expected))
                .unwrap();
            cfb.flush().unwrap();
        }

        let result = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        )
        .unwrap();
        assert_eq!(result.document.bin_streams.len(), 1);
        assert_eq!(result.document.bin_streams[0].data, expected);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn protected_tiny_records_are_refused_before_unbounded_parser_trees() {
        let path = base_hwp("password-tiny-records");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);

        // Each stream is individually below the identity-walk limit, but the
        // authenticated parser shares one record budget across DocInfo and
        // BodyText before it can retain both trees.
        let per_stream = (HWP5_PASSWORD_MAX_RECORDS / 2) + 1;
        replace_encrypted_stream_for_test(
            &path,
            "/DocInfo",
            &repeated_empty_records(crate::record::tag::DOCUMENT_PROPERTIES, per_stream),
            password,
        );
        replace_encrypted_stream_for_test(
            &path,
            "/BodyText/Section0",
            &repeated_empty_records(crate::record::tag::PARA_HEADER, per_stream),
            password,
        );

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::Encrypted)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn aggregate_cfb1_work_is_refused_before_password_transform() {
        let path = base_hwp("password-transform-budget");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut stream = cfb.open_stream("/DocInfo").unwrap();
            stream
                .set_len(password::HWP5_PASSWORD_MAX_TRANSFORM_BYTES + 1)
                .unwrap();
            drop(stream);
            cfb.flush().unwrap();
        }

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::Encrypted)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn authenticated_near_budget_para_text_is_refused_before_semantic_parse() {
        let path = base_hwp("password-near-budget-para-text");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);

        // One UTF-16 unit can retain one HwpChar (two Vec capacities) while
        // three OpaqueRecord/control projections remain live. This payload is
        // derived from those concrete owners, not a stream-size heuristic.
        let bytes_per_input_byte = (size_of::<hwp_model::HwpChar>() as u64)
            .checked_mul(PROTECTED_VEC_GROWTH_BOUND)
            .unwrap()
            / 2
            + PROTECTED_OPAQUE_COPIES_PER_RECORD
            + 2;
        let payload =
            (password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES / bytes_per_input_byte + 2) as usize;
        replace_encrypted_stream_for_test(
            &path,
            "/BodyText/Section0",
            &para_text_record_stream(payload),
            password,
        );

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::Encrypted)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn authenticated_aliased_summary_properties_are_refused_before_semantic_parse() {
        let path = base_hwp("password-aliased-summary");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);
        let summary = aliased_summary_information(8 * 1024 * 1024);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut stream = cfb.open_stream("/\u{5}HwpSummaryInformation").unwrap();
            stream.set_len(0).unwrap();
            stream.write_all(&summary).unwrap();
            drop(stream);
            cfb.flush().unwrap();
        }

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::Encrypted)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn aggregate_path_reservation_rejects_payload_boundary_before_materialization() {
        let name_bytes =
            HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES as usize / HWP5_PASSWORD_MAX_STREAMS;
        let streams: Vec<StreamInfo> = (0..HWP5_PASSWORD_MAX_STREAMS)
            .map(|index| StreamInfo {
                path: format!(
                    "/BinData/{index:04x}{}",
                    "n".repeat(name_bytes.saturating_sub(14))
                ),
                size: 0,
            })
            .collect();
        let path_bytes = password_path_reservation(&streams).unwrap();
        assert!(path_bytes > HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES);
        let retained = password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES - path_bytes + 1;
        assert!(
            password_record_scan_budget(
                retained,
                path_bytes,
                PasswordSemanticReservation::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn nested_cfb_names_block_second_boundary_payload_before_materialization() {
        let path = base_hwp("password-nested-name-boundary");
        let password = "correct-password";
        make_evidenced_password_hwp(&path, password, 0);
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            let mut nested = String::new();
            // 1,024 CFB storages × 31-byte component names make every leaf
            // path about 32 KiB. 512 leaves therefore sit just below the
            // 16 MiB normalized-name contract without relying on host paths.
            for depth in 0..1_024 {
                nested.push('/');
                nested.push_str(&format!("n{depth:04x}{}", "x".repeat(25)));
                cfb.create_storage(&nested).unwrap();
            }
            for index in 0..512 {
                let mut stream = cfb
                    .create_new_stream(format!("{nested}/s{index:04x}"))
                    .unwrap();
                stream.set_len(0).unwrap();
            }
            cfb.create_storage("/BinData").unwrap();
            for name in ["first.bin", "second.bin"] {
                let mut stream = cfb.create_new_stream(format!("/BinData/{name}")).unwrap();
                // Without path accounting both 57 MiB streams fit under the
                // old 128 MiB source-only guard. The second must now fail
                // before `read_stream_raw` materializes it.
                stream.set_len(57 * 1024 * 1024).unwrap();
            }
            cfb.flush().unwrap();
        }

        let error = match read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        ) {
            Ok(_) => panic!("nested names plus second payload must exceed the live bound"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            Hwp5Error::ResourceLimitExceeded { resource, limit }
                if resource == "password-protected HWP5 live buffers"
                    && limit == password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn near_limit_compressed_tiny_records_are_refused_by_the_streaming_walk() {
        let path = base_hwp("password-near-limit-compressed-records");
        let password = "correct-password";
        const COMPRESSED: u32 = 1;
        make_evidenced_password_hwp(&path, password, COMPRESSED);

        // This expands to just under the 64 MiB protected-stream cap but the
        // encrypted representation remains small. The no-allocation identity
        // walk must reject the record count before a RecordNode tree exists.
        let near_limit = repeated_empty_records(
            crate::record::tag::DOCUMENT_PROPERTIES,
            (HWP5_PASSWORD_MAX_STREAM_BYTES as usize / 4) - 1,
        );
        let compressed = crate::codec::compress(&near_limit);
        replace_encrypted_stream_for_test(&path, "/DocInfo", &compressed, password);

        assert!(matches!(
            read_document_with_options(
                &path,
                &ReadOptions {
                    password: Some(password),
                },
            ),
            Err(Hwp5Error::Encrypted)
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
    fn protected_preservation_streams_at_live_boundary_are_moved_into_document() {
        let path = base_hwp("password-preservation-live-boundary");
        let password = "correct-password";
        let protected_record_bytes = {
            let mut container = Hwp5Container::open(&path).unwrap();
            ["/DocInfo", "/BodyText/Section0"]
                .into_iter()
                .map(|stream_path| container.read_record_stream(stream_path).unwrap().len() as u64)
                .sum::<u64>()
        };
        make_evidenced_password_hwp(&path, password, 0);

        // Fill the protected materialization allowance exactly. Before this
        // reader moved source buffers into the final Document, the map and
        // three preservation copies alone exceeded 128 MiB once record trees
        // were constructed. These sparse CFB streams exercise BinData,
        // XMLTemplate, and DocHistory without putting a fixture in git.
        let non_record_bytes = Hwp5Container::open(&path)
            .unwrap()
            .list_streams()
            .iter()
            .filter(|stream| {
                is_password_document_stream(&stream.path)
                    && !is_evidenced_password_record_stream(&stream.path)
            })
            .map(|stream| stream.size)
            .sum::<u64>();
        let existing_bytes = protected_record_bytes + non_record_bytes;
        let allowance = password::HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES / 2;
        assert!(existing_bytes < allowance);
        let opaque_bytes = allowance - existing_bytes;
        let first = opaque_bytes / 3;
        let second = opaque_bytes / 3;
        let third = opaque_bytes - first - second;
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut cfb = cfb::CompoundFile::open(file).unwrap();
            cfb.create_storage("/BinData").unwrap();
            cfb.create_storage("/XMLTemplate").unwrap();
            cfb.create_storage("/DocHistory").unwrap();
            for (stream_path, size) in [
                ("/BinData/boundary.bin", first),
                ("/XMLTemplate/boundary.xml", second),
                ("/DocHistory/boundary.bin", third),
            ] {
                let mut stream = cfb.create_new_stream(stream_path).unwrap();
                stream.set_len(size).unwrap();
            }
            cfb.flush().unwrap();
        }
        assert_eq!(
            protected_record_bytes + non_record_bytes + opaque_bytes,
            allowance
        );

        let result = read_document_with_options(
            &path,
            &ReadOptions {
                password: Some(password),
            },
        )
        .expect("the aggregate boundary must not require preservation copies");
        assert_eq!(result.document.bin_streams.len(), 1);
        assert_eq!(result.document.bin_streams[0].data.len() as u64, first);
        assert_eq!(result.document.hwp5_xml_template.len(), 1);
        assert_eq!(result.document.hwp5_xml_template[0].1.len() as u64, second);
        assert_eq!(result.document.hwp5_doc_history.len(), 1);
        assert_eq!(result.document.hwp5_doc_history[0].1.len() as u64, third);
        drop(result);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn protected_preservation_transfer_reuses_the_source_buffer() {
        let mut streams = BTreeMap::new();
        let data = vec![7u8; 1_024];
        let source_ptr = data.as_ptr();
        streams.insert("/BinData/source.bin".to_string(), data);

        let moved = take_preserved_streams(&mut streams, "/BinData/");
        assert!(streams.is_empty());
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].1.as_ptr(), source_ptr);
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
