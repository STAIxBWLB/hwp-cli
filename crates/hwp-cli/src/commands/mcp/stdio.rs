//! stdio adapter: newline-framed JSON-RPC over stdin/stdout.
//!
//! stdout은 프로토콜 전용이다(라이브러리 함수는 stdout 미오염, 로그는 stderr).
//! 한 줄에 담긴 per-call `password`는 다음 요청을 받기 전에 버퍼째 zeroize한다.

use std::io::{BufRead, Read as _, Write};
use std::path::PathBuf;

use zeroize::{Zeroize as _, Zeroizing};

use super::authority::{LocalFsContext, canonicalize_mcp_path};
use super::{MAX_REQUEST_LINE_BYTES, handle_request};

/// stdio JSON-RPC 루프. EOF까지 한 줄씩 처리한다.
pub fn run(font_dirs: Vec<PathBuf>, roots: Vec<PathBuf>) -> anyhow::Result<()> {
    let mut canonical_roots = Vec::with_capacity(roots.len());
    for root in &roots {
        let canonical = canonicalize_mcp_path(root).map_err(|error| {
            anyhow::anyhow!(
                "--root 경로를 확인할 수 없습니다: {} ({error})",
                root.display()
            )
        })?;
        canonical_roots.push(canonical);
    }
    if canonical_roots.is_empty() {
        eprintln!("경고: --root 미지정 — MCP 서버의 파일 접근이 제한되지 않습니다");
    }
    let ctx = LocalFsContext::new(font_dirs, canonical_roots);
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut line = Zeroizing::new(String::new());
    loop {
        line.clear();
        if read_line_bounded(&mut reader, &mut line, MAX_REQUEST_LINE_BYTES)? == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.zeroize();
            continue;
        }
        if let Some(resp) = handle_request(trimmed, &ctx) {
            out.write_all(resp.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
        // The line can contain the per-call `password`; clear its backing
        // buffer before accepting the next JSON-RPC request.
        line.zeroize();
    }
    Ok(())
}

pub(super) fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut limited = reader.take((max_bytes as u64).saturating_add(1));
    let read = limited.read_line(line)?;
    if line.len() > max_bytes || (read == max_bytes.saturating_add(1) && !line.ends_with('\n')) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MCP 요청 한 줄이 {max_bytes} bytes 제한을 넘었습니다"),
        ));
    }
    Ok(read)
}
