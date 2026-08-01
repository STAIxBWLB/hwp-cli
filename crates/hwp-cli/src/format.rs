//! 파일 포맷 감지.
//!
//! 확장자가 아니라 매직 바이트로 판별한다 — 확장자가 틀린 파일도 처리.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// HWP 5.0 (CFB 컨테이너)
    Hwp5,
    /// HWPX (ZIP + OWPML)
    Hwpx,
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ZIP_MAGIC: [u8; 2] = [0x50, 0x4B]; // "PK"

pub fn detect(path: &Path) -> anyhow::Result<FileFormat> {
    let mut head = [0u8; 8];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .with_context(|| format!("파일을 열 수 없습니다: {}", path.display()))?;

    if n >= 8 && head == CFB_MAGIC {
        Ok(FileFormat::Hwp5)
    } else if n >= 2 && head[..2] == ZIP_MAGIC {
        Ok(FileFormat::Hwpx)
    } else {
        anyhow::bail!(
            "{}: HWP(CFB)도 HWPX(ZIP)도 아닙니다 (시그니처 불일치)",
            path.display()
        )
    }
}

/// 바이트 시그니처로 포맷을 판별해 표준 확장자를 돌려준다 (stdin 스테이징용, GM-2).
pub fn detect_bytes(bytes: &[u8]) -> anyhow::Result<&'static str> {
    if bytes.len() >= 8 && bytes[..8] == CFB_MAGIC {
        Ok("hwp")
    } else if bytes.len() >= 2 && bytes[..2] == ZIP_MAGIC {
        Ok("hwpx")
    } else {
        anyhow::bail!("HWP(CFB)도 HWPX(ZIP)도 아닙니다 (시그니처 불일치)")
    }
}
