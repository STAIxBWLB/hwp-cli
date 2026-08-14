//! 최상위: HWP 5.0 파일 → [`Document`].

use std::collections::BTreeMap;
use std::path::Path;

use hwp_model::{DocMeta, Document};

use crate::body_text::parse_section;
use crate::container::{Hwp5Container, is_record_stream};
use crate::doc_info::parse_doc_info;
use crate::error::{Hwp5Error, Result};
use crate::file_header::FileHeader;
use crate::record::{RecordHeader, ScanMode, scan_stream};

pub struct ReadResult {
    pub document: Document,
    /// 파싱 중 발생한 비치명 경고 (손상/미지원 구조)
    pub warnings: Vec<String>,
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
    };
    Ok(ReadResult { document, warnings })
}

/// HWP 5.0 파일을 IR로 읽는다. 야생 파일 대응을 위해 관용 모드로 스캔한다.
pub fn read_document(path: &Path) -> Result<ReadResult> {
    let mut container = Hwp5Container::open(path)?;
    container.check_body_readable()?;

    let mut warnings = Vec::new();

    // DocInfo
    let doc_info_data = container.read_record_stream("/DocInfo")?;
    let scan = scan_stream(&doc_info_data, ScanMode::Tolerant)?;
    warnings.extend(scan.warnings.iter().map(|w| format!("[DocInfo] {w}")));
    let (header, doc_warnings) = parse_doc_info(&scan.roots);
    warnings.extend(doc_warnings.iter().map(|w| format!("[DocInfo] {w}")));

    // 본문 섹션들
    let mut sections = Vec::new();
    for stream_path in container.body_sections() {
        let data = container.read_record_stream(&stream_path)?;
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
    };
    Ok(ReadResult { document, warnings })
}

#[cfg(test)]
mod bounded_tests {
    use std::io::{Seek as _, SeekFrom, Write as _};
    use std::time::{SystemTime, UNIX_EPOCH};

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
