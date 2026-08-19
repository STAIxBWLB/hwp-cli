//! GATE-01 end-to-end: the distribution-document read path, corpus-gated.
//!
//! This suite CANNOT run in continuous integration: a genuine 배포용문서 cannot be
//! synthesized (the unwrap key schedule is Hancom's), so every test here needs the
//! ground-truth corpus, which lives outside the repository and is never committed.
//! Run it locally with:
//!
//! ```text
//! HWP_CORPUS_DIR=~/Documents/hwp_samples cargo test -p hwp-cli --test input_gate
//! ```
//!
//! The no-skip contract of `cli_surface.rs` is why this is a separate file: CI-safe
//! surface assertions go there, corpus-gated ones go here and skip cleanly.

use std::path::PathBuf;
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

/// 첫 배포용 문서(dist-01*)를 반환. 코퍼스 변수가 없으면 None — 각 테스트는 그 경우
/// 한국어 안내와 함께 조용히 스킵한다.
fn first_distribution_document() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HWP_CORPUS_DIR")?);
    if !dir.is_dir() {
        return None;
    }
    let mut docs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            let name = p.file_name()?.to_string_lossy();
            (name.starts_with("dist-") && name.ends_with(".hwp")).then_some(p)
        })
        .collect();
    docs.sort();
    docs.into_iter().next()
}

macro_rules! require_corpus {
    () => {
        match first_distribution_document() {
            Some(doc) => doc,
            None => {
                eprintln!(
                    "skip: HWP_CORPUS_DIR 미설정 — 배포용 문서 경로는 실물 코퍼스로만 검증 가능"
                );
                return;
            }
        }
    };
}

const UNWRAP_NOTICE: &str = "배포용 문서를 해제했습니다";
const WRITE_BACK_WARNING: &str = "배포용 보호는 유지되지 않습니다";

/// stdout은 문서 텍스트만 담는다 — 안내·경고가 한 바이트라도 섞이면 파이프 소비자가
/// 깨진다(D-03).
#[test]
fn distribution_document_text_goes_to_stdout_only() {
    let doc = require_corpus!();
    let r = hwp().arg("cat").arg(&doc).output().unwrap();
    assert!(
        r.status.success(),
        "cat: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(!stdout.trim().is_empty(), "본문 텍스트가 비어 있으면 안 됨");
    assert!(
        !stdout.contains(UNWRAP_NOTICE),
        "해제 안내가 stdout에 새어나감"
    );
    assert!(
        !stdout.contains(WRITE_BACK_WARNING),
        "쓰기 경고가 stdout에 새어나감"
    );
}

/// 해제 사실은 stderr 한 줄로 알린다(D-10a: 경고까지 합친 정확히 한 줄).
#[test]
fn distribution_document_unwrap_is_announced_on_stderr() {
    let doc = require_corpus!();
    let r = hwp().arg("cat").arg(&doc).output().unwrap();
    assert!(r.status.success());
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains(UNWRAP_NOTICE),
        "해제 안내 부재. stderr: {stderr}"
    );
    let non_empty = stderr.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(non_empty, 1, "stderr는 정확히 한 줄이어야 함: {stderr}");
}

/// 같은 한 줄에 쓰기 경고가 함께 실린다 — 다른 형식으로 변환해 저장하면 배포용
/// 보호가 유지되지 않는다는 사실(02-04 Task 0 실측 기반 문구).
#[test]
fn distribution_document_write_back_is_warned_about() {
    let doc = require_corpus!();
    let r = hwp().arg("cat").arg(&doc).output().unwrap();
    assert!(r.status.success());
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains(WRITE_BACK_WARNING),
        "쓰기 경고 부재. stderr: {stderr}"
    );
}
