//! `hwp dump` — 레코드/패키지 구조 덤프 (개발자용).
//!
//! - HWP 5.0: 지정 스트림(기본 DocInfo + 모든 BodyText 섹션)을 압축 해제해
//!   레코드 트리를 출력한다. 야생 파일 진단이 목적이므로 관용 모드로 스캔하고
//!   경고는 stderr로 보낸다.
//! - HWPX: 지정 엔트리 내용을 출력하거나(생략 시) 엔트리 목록을 보여준다.

use std::path::Path;

use hwp5::record::{RecordNode, ScanMode, scan_stream, tag};
use serde_json::json;

use crate::format::{FileFormat, detect};

pub fn run(path: &Path, stream: Option<&str>, raw: bool, as_json: bool) -> anyhow::Result<()> {
    match detect(path)? {
        FileFormat::Hwp5 => dump_hwp5(path, stream, raw, as_json),
        FileFormat::Hwpx => dump_hwpx(path, stream, raw),
    }
}

fn dump_hwp5(path: &Path, stream: Option<&str>, raw: bool, as_json: bool) -> anyhow::Result<()> {
    let mut container = hwp5::Hwp5Container::open(path)?;
    container.check_body_readable()?;
    // `dump`'s purpose is showing the *raw* record tree; the decrypted
    // ViewText bytes are a derived artifact, not the stream's literal
    // content, so this command keeps refusing distribution documents even
    // though GATE-01 makes `cat`/`convert`/`render` read them.
    if container.file_header().is_distribution() {
        return Err(hwp5::Hwp5Error::DistributionDoc.into());
    }

    // 대상 스트림 결정: 지정이 없으면 DocInfo + 모든 본문 섹션
    let targets: Vec<String> = match stream {
        Some(s) => vec![normalize_stream_path(s)],
        None => {
            let mut v = vec!["/DocInfo".to_string()];
            v.extend(container.body_sections());
            v
        }
    };

    let mut json_streams = Vec::new();
    for target in &targets {
        let data = container.read_record_stream(target)?;
        let result = scan_stream(&data, ScanMode::Tolerant)?;
        for w in &result.warnings {
            eprintln!("경고 [{target}]: {w}");
        }

        if as_json {
            json_streams.push(json!({
                "stream": target,
                "record_count": result.record_count,
                "records": result.roots.iter().map(|n| node_to_json(n, raw)).collect::<Vec<_>>(),
            }));
        } else {
            println!("── {} (레코드 {}개) ──", target, result.record_count);
            for node in &result.roots {
                print_node(node, 0, raw);
            }
            println!();
        }
    }

    if as_json {
        let v = json!({
            "file": path.display().to_string(),
            "format": "hwp5",
            "streams": json_streams,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}

/// "DocInfo"나 "BodyText/Section0" 같은 입력을 CFB 경로로 정규화.
fn normalize_stream_path(s: &str) -> String {
    if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{s}")
    }
}

fn print_node(node: &RecordNode, depth: usize, raw: bool) {
    let indent = "  ".repeat(depth);
    let name = tag::tag_name(node.tag).unwrap_or("UNKNOWN");
    println!("{indent}{name} (0x{:03X}) {} B", node.tag, node.data.len());
    if raw && !node.data.is_empty() {
        for line in hex_lines(&node.data) {
            println!("{indent}    {line}");
        }
    }
    for child in &node.children {
        print_node(child, depth + 1, raw);
    }
}

fn node_to_json(node: &RecordNode, raw: bool) -> serde_json::Value {
    let mut v = json!({
        "tag": format!("0x{:03X}", node.tag),
        "name": tag::tag_name(node.tag),
        "size": node.data.len(),
        "children": node.children.iter().map(|c| node_to_json(c, raw)).collect::<Vec<_>>(),
    });
    if raw {
        v.as_object_mut()
            .expect("json! 매크로는 객체를 생성")
            .insert("data_hex".to_string(), json!(hex_string(&node.data)));
    }
    v
}

fn dump_hwpx(path: &Path, stream: Option<&str>, raw: bool) -> anyhow::Result<()> {
    let mut pkg = hwpx::HwpxPackage::open(path)?;
    match stream {
        None => {
            println!("엔트리 목록 (--stream <이름>으로 내용 덤프):");
            for e in pkg.entries()? {
                println!("  {:<40} {:>10} B", e.name, e.size);
            }
        }
        Some(name) => {
            let data = pkg.read_entry(name)?;
            if raw || std::str::from_utf8(&data).is_err() {
                for line in hex_lines(&data) {
                    println!("{line}");
                }
            } else {
                println!("{}", String::from_utf8_lossy(&data));
            }
        }
    }
    Ok(())
}

fn hex_string(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// 16바이트 단위 hex 덤프 라인.
fn hex_lines(data: &[u8]) -> Vec<String> {
    data.chunks(16)
        .enumerate()
        .map(|(i, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            format!("{:08x}  {}", i * 16, hex.join(" "))
        })
        .collect()
}
