//! HWP 스트림 압축/해제.
//!
//! FileHeader의 압축 플래그(bit 0)가 설정된 경우 DocInfo/BodyText 스트림은
//! **zlib 헤더 없는 raw deflate**로 압축되어 있다 (pyhwp의 wbits=-15와 동일).
//! Scripts 스트림(DefaultJScript 등)도 압축 대상이다 — 정품 표본 실측
//! (문서 10 §1 쓰기 비대칭 참조).

use std::io::Read;

use flate2::Compression;
use flate2::read::{DeflateDecoder, DeflateEncoder};

use crate::error::{Hwp5Error, Result};

/// raw deflate 해제. `stream_name`은 오류 메시지용.
pub fn decompress(data: &[u8], stream_name: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    DeflateDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|source| Hwp5Error::Decompress {
            stream: stream_name.to_string(),
            source,
        })?;
    Ok(out)
}

/// Raw deflate decode with an allocation bound. The decoder is stopped after
/// `limit + 1` bytes so callers can reject compressed streams before semantic parsing.
pub fn decompress_bounded(data: &[u8], stream_name: &str, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    DeflateDecoder::new(data)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|source| Hwp5Error::Decompress {
            stream: stream_name.to_string(),
            source,
        })?;
    if out.len() as u64 > limit {
        return Err(Hwp5Error::ResourceLimitExceeded {
            resource: format!("{stream_name} decompressed stream"),
            limit,
        });
    }
    Ok(out)
}

/// raw deflate 압축 (writer 경로용).
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    DeflateEncoder::new(data, Compression::default())
        .read_to_end(&mut out)
        .expect("메모리 버퍼 deflate 압축은 실패하지 않는다");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 압축_해제_왕복() {
        let original = "한글 문서 파일 형식 5.0".repeat(100);
        let packed = compress(original.as_bytes());
        assert!(packed.len() < original.len());
        let unpacked = decompress(&packed, "test").unwrap();
        assert_eq!(unpacked, original.as_bytes());
    }

    #[test]
    fn 잘못된_데이터는_err() {
        assert!(decompress(&[0xFF, 0xFF, 0xFF, 0xFF], "test").is_err());
    }

    #[test]
    fn bounded_압축_해제는_limit_plus_one에서_중단() {
        let packed = compress(&vec![0; 1024]);
        assert!(decompress_bounded(&packed, "test", 1023).is_err());
        assert_eq!(
            decompress_bounded(&packed, "test", 1024).unwrap().len(),
            1024
        );
    }
}
