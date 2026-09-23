//! `hwp info` — 컨테이너 계층만으로 동작하는 파일 진단.
//!
//! 본문 파싱에 실패하는(손상/미지원) 파일도 진단할 수 있어야 하므로
//! 레코드 해석에 의존하지 않는다.

use std::path::Path;

use hwp_model::Metadata;
use serde_json::{Value, json};

use crate::format::{FileFormat, detect};

/// 메타데이터를 JSON 객체로 (모든 필드가 비면 `null`).
fn metadata_json(meta: &Metadata) -> Value {
    if meta.is_empty() {
        return Value::Null;
    }
    json!({
        "title": meta.title,
        "author": meta.author,
        "subject": meta.subject,
        "keywords": meta.keywords,
    })
}

/// 문서 메타데이터를 본문 파싱 없이 최선 노력으로 읽는다(요약 정보 / content.hpf만).
fn read_metadata(path: &Path, format: FileFormat) -> Metadata {
    match format {
        FileFormat::Hwp5 => hwp5::Hwp5Container::open(path)
            .ok()
            .and_then(|mut c| c.read_stream_raw("/\u{5}HwpSummaryInformation").ok())
            .map(|raw| hwp5::parse_summary(&raw))
            .unwrap_or_default(),
        FileFormat::Hwpx => hwpx::HwpxPackage::open(path)
            .ok()
            .and_then(|mut p| p.read_entry_string("Contents/content.hpf").ok())
            .map(|xml| hwpx::parse_content_meta(&xml))
            .unwrap_or_default(),
    }
}

/// 컨테이너 계층 진단을 JSON으로 만든다 (CLI `--json`과 MCP가 공유).
pub fn info_json(path: &Path) -> anyhow::Result<Value> {
    match detect(path)? {
        FileFormat::Hwp5 => {
            let container = hwp5::Hwp5Container::open(path)?;
            let header = container.file_header();
            let streams = container.list_streams();
            Ok(json!({
                "file": path.display().to_string(),
                "format": "hwp5",
                "version": header.version.to_string(),
                "attributes": header.attribute_names(),
                "compressed": header.is_compressed(),
                "encrypted": header.is_encrypted(),
                "distribution": header.is_distribution(),
                "sections": container.body_sections().len(),
                "metadata": metadata_json(&read_metadata(path, FileFormat::Hwp5)),
                "streams": streams.iter().map(|s| json!({
                    "path": s.path,
                    "size": s.size,
                })).collect::<Vec<_>>(),
            }))
        }
        FileFormat::Hwpx => {
            let mut pkg = hwpx::HwpxPackage::open(path)?;
            let version = pkg.version_info()?;
            let entries = pkg.entries()?;
            let sections = pkg.section_entries()?;
            Ok(json!({
                "file": path.display().to_string(),
                "format": "hwpx",
                "version": version.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
                "sections": sections.len(),
                "metadata": metadata_json(&read_metadata(path, FileFormat::Hwpx)),
                "entries": entries.iter().map(|e| json!({
                    "name": e.name,
                    "size": e.size,
                    "compressed_size": e.compressed_size,
                })).collect::<Vec<_>>(),
            }))
        }
    }
}

/// 메타데이터를 사람용 텍스트로 출력 (비어 있으면 아무것도 출력 안 함).
fn print_metadata(meta: &Metadata) {
    let rows = [
        ("제목", &meta.title),
        ("지은이", &meta.author),
        ("주제", &meta.subject),
        ("키워드", &meta.keywords),
    ];
    for (label, val) in rows {
        if let Some(v) = val.as_deref().filter(|v| !v.is_empty()) {
            println!("{label}:   {v}");
        }
    }
}

pub fn run(path: &Path, as_json: bool, body_stats: bool) -> anyhow::Result<()> {
    if as_json {
        let mut out = info_json(path)?;
        if body_stats {
            let (total, inline, floating) = body_table_stats(path)?;
            out["body"] = json!({
                "tables": total,
                "tables_inline": inline,
                "tables_floating": floating,
            });
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    match detect(path)? {
        FileFormat::Hwp5 => info_hwp5_text(path)?,
        FileFormat::Hwpx => info_hwpx_text(path)?,
    }
    if body_stats {
        let (total, inline, floating) = body_table_stats(path)?;
        println!("표:     {total}개 (인라인 {inline}, 부유 {floating})");
    }
    Ok(())
}

/// `--body-stats` (#296): 본문을 파싱해 표 배치(글자처럼 취급) 수를 센다. 기본 info는
/// 컨테이너 계층만 읽는 계약(모듈 주석 참조)이라 opt-in 플래그로만 동작한다.
fn body_table_stats(path: &Path) -> anyhow::Result<(usize, usize, usize)> {
    let mut doc = crate::commands::cat::load_document(path)?;
    Ok(hwp_convert::table_placement_stats(&mut doc))
}

fn info_hwp5_text(path: &Path) -> anyhow::Result<()> {
    let container = hwp5::Hwp5Container::open(path)?;
    let header = container.file_header();
    let streams = container.list_streams();

    println!("파일:   {}", path.display());
    println!("포맷:   HWP 5.0 (바이너리)");
    println!("버전:   {}", header.version);
    let attrs = header.attribute_names();
    println!(
        "속성:   {}",
        if attrs.is_empty() {
            "(없음)".to_string()
        } else {
            attrs.join(", ")
        }
    );
    println!("섹션:   {}개", container.body_sections().len());
    print_metadata(&read_metadata(path, FileFormat::Hwp5));
    println!("스트림: {}개", streams.len());
    for s in &streams {
        println!("  {:<40} {:>10} B", printable(&s.path), s.size);
    }
    Ok(())
}

fn info_hwpx_text(path: &Path) -> anyhow::Result<()> {
    let mut pkg = hwpx::HwpxPackage::open(path)?;
    let version = pkg.version_info()?;
    let entries = pkg.entries()?;
    let sections = pkg.section_entries()?;

    println!("파일:   {}", path.display());
    println!("포맷:   HWPX (OWPML)");
    if !version.is_empty() {
        let pairs: Vec<String> = version.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("버전:   {}", pairs.join(" "));
    }
    println!("섹션:   {}개", sections.len());
    print_metadata(&read_metadata(path, FileFormat::Hwpx));
    println!("엔트리: {}개", entries.len());
    for e in &entries {
        println!("  {:<40} {:>10} B", e.name, e.size);
    }
    Ok(())
}

/// 제어 문자가 들어간 스트림 이름(`\x05HwpSummaryInformation`)을 표시 가능하게.
fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
