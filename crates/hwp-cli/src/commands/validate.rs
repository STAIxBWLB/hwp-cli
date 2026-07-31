//! `hwp validate` — 구조 검증.
//!
//! mimetype·필수 엔트리 존재·XML 파싱(본문/헤더)을 확인한다. XSD 스키마 검증은
//! 범위 밖. 하드 오류가 하나라도 있으면 비-0 종료코드(소비자가 exit code로 판정).

use std::path::Path;

use serde_json::{Value, json};

use crate::format::{FileFormat, detect};

/// 구조 검증 결과를 JSON 객체로 만든다 (CLI와 MCP가 공유 — `process::exit` 없음).
/// `{file, format, valid, errors, warnings}`.
pub fn validate_json(path: &Path) -> Value {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut format = "unknown";
    let mut package_limits = None;

    match detect(path) {
        Ok(FileFormat::Hwpx) => {
            format = "hwpx";
            package_limits = Some(json!(hwpx::NATIVE_PACKAGE_LIMITS_PROFILE));
            validate_hwpx(path, &mut errors, &mut warnings);
        }
        Ok(FileFormat::Hwp5) => {
            format = "hwp5";
            match hwp5::read_document(path) {
                Ok(r) => warnings.extend(r.warnings),
                Err(e) => errors.push(format!("파싱 실패: {e}")),
            }
        }
        Err(e) => errors.push(format!("포맷 감지 실패: {e}")),
    }

    json!({
        "file": path.display().to_string(),
        "format": format,
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "package_limits": package_limits,
    })
}

pub fn run(path: &Path, json: bool) -> anyhow::Result<()> {
    let result = validate_json(path);
    let valid = result["valid"].as_bool().unwrap_or(false);

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("파일: {}", path.display());
        println!("포맷: {}", result["format"].as_str().unwrap_or("unknown"));
        println!("결과: {}", if valid { "유효" } else { "오류" });
        for e in result["errors"].as_array().into_iter().flatten() {
            println!("  오류: {}", e.as_str().unwrap_or_default());
        }
        for w in result["warnings"].as_array().into_iter().flatten() {
            println!("  경고: {}", w.as_str().unwrap_or_default());
        }
    }

    if valid {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn validate_hwpx(path: &Path, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    let mut pkg = match hwpx::HwpxPackage::open(path) {
        Ok(p) => p,
        Err(e) => {
            errors.push(format!("패키지/mimetype 오류: {e}"));
            return;
        }
    };

    let names: Vec<String> = match pkg.entries() {
        Ok(es) => es.into_iter().map(|e| e.name).collect(),
        Err(e) => {
            errors.push(format!("엔트리 목록 실패: {e}"));
            return;
        }
    };
    for req in [
        "mimetype",
        "version.xml",
        "Contents/header.xml",
        "META-INF/container.xml",
    ] {
        if !names.iter().any(|n| n == req) {
            errors.push(format!("필수 엔트리 누락: {req}"));
        }
    }
    if !names.iter().any(|n| n.starts_with("Contents/section")) {
        errors.push("본문 섹션(Contents/section*.xml) 없음".to_string());
    }
    // 패키징 규칙: mimetype이 첫 엔트리여야 한다(write 경로가 강제하는 불변식). 리더는
    // 이름으로 읽어 순서가 어긋나도 열리므로 하드 에러 대신 경고로만 표면화한다.
    if names.first().map(String::as_str) != Some("mimetype") {
        warnings.push("mimetype이 첫 엔트리가 아님 (패키징 규칙 위반)".to_string());
    }

    // 구조 검증은 BinData를 메모리에 적재하지 않지만 모든 비디렉터리 엔트리를
    // EOF까지 스트리밍한다. 따라서 raw-copy 대상의 CRC/deflate 손상도 검출한다.
    match hwpx::read_structure(path) {
        Ok(r) => warnings.extend(r.warnings),
        Err(e) => errors.push(format!("XML 파싱 실패: {e}")),
    }
}
