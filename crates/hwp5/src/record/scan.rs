//! 레코드 스트림 스캐너.
//!
//! 압축 해제된 DocInfo/BodyText 스트림을 (tag, level, data) 평면 목록으로
//! 읽고 트리로 복원한다. 태그는 해석하지 않는다.

use crate::codec::ByteReader;
use crate::error::{Hwp5Error, Result};
use crate::record::header::RecordHeader;
use crate::record::tree::RecordNode;

/// A cumulative budget for record-tree construction. Byte length alone is not
/// sufficient because empty records still allocate vectors and tree nodes.
#[derive(Debug, Clone, Copy)]
pub struct RecordScanLimits {
    pub max_records: usize,
    pub max_depth: usize,
    pub max_allocation_bytes: u64,
}

/// Mutable accounting shared by every record stream parsed in one document.
#[derive(Debug)]
pub struct RecordScanBudget {
    limits: RecordScanLimits,
    records: usize,
    allocation_bytes: u64,
}

impl RecordScanBudget {
    pub fn new(limits: RecordScanLimits) -> Self {
        Self {
            limits,
            records: 0,
            allocation_bytes: 0,
        }
    }

    fn reserve_record(&mut self, header: RecordHeader) -> Result<()> {
        self.records =
            self.records
                .checked_add(1)
                .ok_or_else(|| Hwp5Error::StructureLimitExceeded {
                    resource: "aggregate record count".to_string(),
                    limit: self.limits.max_records,
                })?;
        if self.records > self.limits.max_records {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "aggregate record count".to_string(),
                limit: self.limits.max_records,
            });
        }
        if usize::from(header.level) > self.limits.max_depth {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "record nesting depth".to_string(),
                limit: self.limits.max_depth,
            });
        }

        // This covers a flat entry, a RecordNode, backing-vector growth, and
        // a tolerant-mode warning even when the payload is empty.
        const PER_RECORD_OVERHEAD: u64 = 512;
        let record_bytes = u64::from(header.size)
            .checked_add(PER_RECORD_OVERHEAD)
            .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                resource: "record parser allocation".to_string(),
                limit: self.limits.max_allocation_bytes,
            })?;
        self.allocation_bytes =
            self.allocation_bytes
                .checked_add(record_bytes)
                .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                    resource: "record parser allocation".to_string(),
                    limit: self.limits.max_allocation_bytes,
                })?;
        if self.allocation_bytes > self.limits.max_allocation_bytes {
            return Err(Hwp5Error::ResourceLimitExceeded {
                resource: "record parser allocation".to_string(),
                limit: self.limits.max_allocation_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// 손상 발견 시 즉시 Err — writer 검증·왕복 테스트용.
    Strict,
    /// 가능한 만큼 읽고 경고 누적 — 야생 파일 진단용.
    Tolerant,
}

#[derive(Debug)]
pub struct ScanResult {
    pub roots: Vec<RecordNode>,
    pub warnings: Vec<String>,
    /// 스캔한 레코드 총 수.
    pub record_count: usize,
}

/// 압축 해제된 레코드 스트림을 스캔해 트리로 복원한다.
pub fn scan_stream(data: &[u8], mode: ScanMode) -> Result<ScanResult> {
    scan_stream_with_budget(data, mode, None)
}

/// Scans a record stream while enforcing a cumulative document budget before
/// each payload clone or `RecordNode` allocation.
pub fn scan_stream_bounded(
    data: &[u8],
    mode: ScanMode,
    budget: &mut RecordScanBudget,
) -> Result<ScanResult> {
    scan_stream_with_budget(data, mode, Some(budget))
}

fn scan_stream_with_budget(
    data: &[u8],
    mode: ScanMode,
    mut budget: Option<&mut RecordScanBudget>,
) -> Result<ScanResult> {
    let mut r = ByteReader::new(data);
    let mut flat: Vec<(RecordHeader, Vec<u8>)> = Vec::new();
    let mut warnings = Vec::new();

    while !r.is_empty() {
        let at = r.pos();
        let header = match RecordHeader::decode(&mut r) {
            Ok(h) => h,
            Err(e) => match mode {
                ScanMode::Strict => return Err(e),
                ScanMode::Tolerant => {
                    warnings.push(format!("오프셋 {at}: 레코드 헤더가 잘림 — 스캔 중단"));
                    break;
                }
            },
        };
        if let Some(budget) = budget.as_deref_mut() {
            budget.reserve_record(header)?;
        }
        let payload = match r.read_bytes(header.size as usize) {
            Ok(b) => b.to_vec(),
            Err(_) => match mode {
                ScanMode::Strict => {
                    return Err(Hwp5Error::MalformedRecord(format!(
                        "오프셋 {at}: tag 0x{:03X}의 페이로드 {}바이트 중 {}바이트만 남음",
                        header.tag,
                        header.size,
                        r.remaining(),
                    )));
                }
                ScanMode::Tolerant => {
                    warnings.push(format!(
                        "오프셋 {at}: tag 0x{:03X} 페이로드 잘림({}바이트 요구, {}바이트 잔여) — 잘린 채 보존",
                        header.tag,
                        header.size,
                        r.remaining(),
                    ));
                    r.take_rest().to_vec()
                }
            },
        };
        flat.push((header, payload));
    }

    let record_count = flat.len();
    let (roots, tree_warnings) = RecordNode::build_forest(flat);
    if mode == ScanMode::Strict && !tree_warnings.is_empty() {
        return Err(Hwp5Error::MalformedRecord(tree_warnings.join("; ")));
    }
    warnings.extend(tree_warnings);

    Ok(ScanResult {
        roots,
        warnings,
        record_count,
    })
}

/// Performs a strict record-header walk without constructing payload vectors
/// or a record tree. Password authentication uses this before normal parsing
/// so a candidate cannot force allocations with many tiny records.
pub fn walk_stream_strict(
    data: &[u8],
    expected_tag: u16,
    max_records: usize,
    max_depth: usize,
) -> Result<bool> {
    let mut reader = ByteReader::new(data);
    let mut record_count = 0usize;
    let mut open_depth = 0usize;
    let mut found_expected_tag = false;

    while !reader.is_empty() {
        let header = RecordHeader::decode(&mut reader)?;
        record_count =
            record_count
                .checked_add(1)
                .ok_or_else(|| Hwp5Error::StructureLimitExceeded {
                    resource: "record count".to_string(),
                    limit: max_records,
                })?;
        if record_count > max_records {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "record count".to_string(),
                limit: max_records,
            });
        }
        let level = usize::from(header.level);
        if level > max_depth {
            return Err(Hwp5Error::StructureLimitExceeded {
                resource: "record nesting depth".to_string(),
                limit: max_depth,
            });
        }
        if level > open_depth {
            return Err(Hwp5Error::MalformedRecord(format!(
                "record level {level} exceeds open tree depth {open_depth}"
            )));
        }
        open_depth = level + 1;
        reader.read_bytes(header.size as usize)?;
        found_expected_tag |= header.tag == expected_tag;
    }

    Ok(found_expected_tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ByteWriter;

    fn emit(records: &[(u16, u16, &[u8])]) -> Vec<u8> {
        let mut w = ByteWriter::new();
        for (tag, level, data) in records {
            RecordHeader {
                tag: *tag,
                level: *level,
                size: data.len() as u32,
            }
            .encode(&mut w);
            w.write_bytes(data);
        }
        w.into_bytes()
    }

    #[test]
    fn 정상_스트림_스캔() {
        let bytes = emit(&[(0x10, 0, b"abc"), (0x11, 1, b""), (0x12, 0, b"de")]);
        let res = scan_stream(&bytes, ScanMode::Strict).unwrap();
        assert_eq!(res.record_count, 3);
        assert_eq!(res.roots.len(), 2);
        assert!(res.warnings.is_empty());
    }

    #[test]
    fn 잘린_스트림은_strict에서_err_tolerant에서_경고() {
        let mut bytes = emit(&[(0x10, 0, b"abcdef")]);
        bytes.truncate(bytes.len() - 3); // 페이로드 절단
        assert!(scan_stream(&bytes, ScanMode::Strict).is_err());
        let res = scan_stream(&bytes, ScanMode::Tolerant).unwrap();
        assert_eq!(res.record_count, 1);
        assert_eq!(res.warnings.len(), 1);
    }

    #[test]
    fn 빈_스트림() {
        let res = scan_stream(&[], ScanMode::Strict).unwrap();
        assert_eq!(res.record_count, 0);
        assert!(res.roots.is_empty());
    }

    #[test]
    fn bounded_scan_rejects_tiny_record_allocation_bomb_before_tree_build() {
        let bytes = emit(&[(1, 0, b""), (1, 0, b""), (1, 0, b"")]);
        let mut budget = RecordScanBudget::new(RecordScanLimits {
            max_records: 2,
            max_depth: 8,
            max_allocation_bytes: 1_024,
        });
        assert!(matches!(
            scan_stream_bounded(&bytes, ScanMode::Strict, &mut budget),
            Err(Hwp5Error::StructureLimitExceeded { resource, limit: 2 })
                if resource == "aggregate record count"
        ));
    }

    #[test]
    fn strict_header_walk_checks_identity_without_constructing_a_tree() {
        let bytes = emit(&[(1, 0, b""), (2, 0, b""), (3, 0, b"")]);
        assert!(walk_stream_strict(&bytes, 2, 3, 8).unwrap());
        assert!(!walk_stream_strict(&bytes, 9, 3, 8).unwrap());
        assert!(walk_stream_strict(&bytes, 1, 2, 8).is_err());
    }
}
