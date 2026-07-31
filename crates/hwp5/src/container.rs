//! CFB 컨테이너 래핑.
//!
//! HWP 5.0 파일을 열고 FileHeader를 검증한 뒤, 스트림 열거·읽기와
//! 압축 해제(레코드 스트림 한정)를 제공한다.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::codec;
use crate::error::{Hwp5Error, Result};
use crate::file_header::FileHeader;

/// 스트림 메타데이터 (`hwp info`용).
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// CFB 내부 경로 (예: `/BodyText/Section0`).
    pub path: String,
    pub size: u64,
}

pub struct Hwp5Container {
    cfb: cfb::CompoundFile<File>,
    header: FileHeader,
}

impl Hwp5Container {
    /// 파일을 열고 FileHeader를 검증한다.
    ///
    /// CFB가 아니거나 FileHeader 스트림이 없으면 [`Hwp5Error::NotHwp5`].
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut cfb = cfb::CompoundFile::open(file).map_err(|_| Hwp5Error::NotHwp5)?;
        let mut raw = Vec::new();
        cfb.open_stream("/FileHeader")
            .map_err(|_| Hwp5Error::NotHwp5)?
            .read_to_end(&mut raw)?;
        let header = FileHeader::parse(&raw)?;
        Ok(Self { cfb, header })
    }

    pub fn file_header(&self) -> &FileHeader {
        &self.header
    }

    /// 모든 스트림을 경로순으로 열거한다.
    pub fn list_streams(&self) -> Vec<StreamInfo> {
        let mut v: Vec<StreamInfo> = self
            .cfb
            .walk()
            .filter(|e| e.is_stream())
            .map(|e| StreamInfo {
                // cfb의 Entry::path()는 Path::join으로 만드므로 Windows에서는 구분자가
                // `\`로 렌더된다 — 스트림 경로 필터(`/BodyText/Section`)와 open_stream이
                // `/`를 전제하므로 항상 `/`로 정규화한다(CFB 항목명 자체엔 구분자가 못 옴).
                path: e.path().to_string_lossy().replace('\\', "/"),
                size: e.len(),
            })
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    /// Certification-oriented stream enumeration. The entry and normalized
    /// name budgets are enforced while walking the CFB directory, before a
    /// caller can clone all paths into a stream cache.
    pub fn list_streams_bounded(
        &self,
        max_streams: usize,
        max_total_name_bytes: u64,
    ) -> Result<Vec<StreamInfo>> {
        let mut streams = Vec::new();
        let mut total_name_bytes = 0u64;
        for entry in self.cfb.walk().filter(|entry| entry.is_stream()) {
            if streams.len() >= max_streams {
                return Err(Hwp5Error::StructureLimitExceeded {
                    resource: "CFB stream count".to_string(),
                    limit: max_streams,
                });
            }
            let path = entry.path().to_string_lossy().replace('\\', "/");
            total_name_bytes =
                total_name_bytes
                    .checked_add(path.len() as u64)
                    .ok_or_else(|| Hwp5Error::ResourceLimitExceeded {
                        resource: "aggregate CFB stream names".to_string(),
                        limit: max_total_name_bytes,
                    })?;
            if total_name_bytes > max_total_name_bytes {
                return Err(Hwp5Error::ResourceLimitExceeded {
                    resource: "aggregate CFB stream names".to_string(),
                    limit: max_total_name_bytes,
                });
            }
            streams.push(StreamInfo {
                path,
                size: entry.len(),
            });
        }
        streams.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(streams)
    }

    /// 본문 섹션 스트림 경로 목록 (`/BodyText/Section0`, `/BodyText/Section1`, …).
    pub fn body_sections(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .list_streams()
            .into_iter()
            .map(|s| s.path)
            .filter(|p| p.starts_with("/BodyText/Section"))
            .collect();
        // Section10이 Section2보다 뒤에 오도록 번호 기준 정렬
        v.sort_by_key(|p| {
            p.trim_start_matches("/BodyText/Section")
                .parse::<u32>()
                .unwrap_or(u32::MAX)
        });
        v
    }

    /// 스트림 원본 바이트를 그대로 읽는다 (압축 해제 없음).
    pub fn read_stream_raw(&mut self, path: &str) -> Result<Vec<u8>> {
        let mut stream = self
            .cfb
            .open_stream(path)
            .map_err(|_| Hwp5Error::StreamNotFound(path.to_string()))?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// 레코드 스트림(DocInfo/BodyText/Scripts)을 읽는다.
    /// FileHeader의 압축 플래그가 설정되어 있으면 raw deflate를 해제한다.
    pub fn read_record_stream(&mut self, path: &str) -> Result<Vec<u8>> {
        let raw = self.read_stream_raw(path)?;
        if self.header.is_compressed() && is_record_stream(path) {
            codec::decompress(&raw, path)
        } else {
            Ok(raw)
        }
    }

    /// Certification-oriented record read that bounds both the stored stream and
    /// the raw-deflate output before allocating the semantic record tree.
    pub fn read_record_stream_bounded(&mut self, path: &str, limit: u64) -> Result<Vec<u8>> {
        let info = self
            .list_streams()
            .into_iter()
            .find(|info| info.path == path)
            .ok_or_else(|| Hwp5Error::StreamNotFound(path.to_string()))?;
        if info.size > limit {
            return Err(Hwp5Error::ResourceLimitExceeded {
                resource: format!("{path} stored stream"),
                limit,
            });
        }
        let raw = self.read_stream_raw(path)?;
        if self.header.is_compressed() && is_record_stream(path) {
            codec::decompress_bounded(&raw, path, limit)
        } else if raw.len() as u64 > limit {
            Err(Hwp5Error::ResourceLimitExceeded {
                resource: format!("{path} record stream"),
                limit,
            })
        } else {
            Ok(raw)
        }
    }

    /// 미지원 버전/배포용/암호화 문서면 본문 접근 전에 명확한 에러를 낸다.
    pub fn check_body_readable(&self) -> Result<()> {
        self.header.check_version()?;
        if self.header.is_encrypted() {
            return Err(Hwp5Error::Encrypted);
        }
        if self.header.is_distribution() {
            return Err(Hwp5Error::DistributionDoc);
        }
        Ok(())
    }
}

/// 압축 플래그의 적용을 받는 레코드 스트림인지 판별한다.
/// (FileHeader, PrvText, PrvImage, BinData, 요약 정보 등은 압축 플래그와 무관)
pub fn is_record_stream(path: &str) -> bool {
    path == "/DocInfo"
        || path.starts_with("/BodyText/")
        || path.starts_with("/ViewText/")
        // Scripts 스트림도 압축 대상(정품 표본 실측 — 문서 10 §1 쓰기 비대칭 참조).
        || path.starts_with("/Scripts/")
}
