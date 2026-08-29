//! Pins the MCP `tools/list` schema against drift.
//!
//! The tool schemas are the wire contract two adapters share and three doc surfaces describe, and
//! nothing else compares them as a whole: `cli_surface.rs` and `serve_http.rs` assert tool *names*,
//! and the in-module tests assert a handful of individual properties. A change to any other
//! property currently lands silently.
//!
//! The snapshot drives a real `hwp mcp` child process rather than calling `tool_defs()`, so what is
//! pinned is the bytes a client actually receives. Only `result.tools` is captured — never
//! `initialize`, whose `serverInfo.version` moves with every release.
//!
//! Bless: `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test mcp_schema`.

use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Committed snapshot path (relative to the crate).
fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/mcp-tools-list.json")
}

/// Runs `hwp mcp` over stdio and returns the `tools/list` result array.
///
/// Receive is a thread plus `recv_timeout`, and shutdown is stdin EOF then a `try_wait` loop, both
/// copied from `cli_surface.rs::mcp_stdio_session` — the shape that keeps CI from hanging.
fn tools_list() -> serde_json::Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("hwp mcp spawn");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let recv = |what: &str| -> serde_json::Value {
        let line = rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("MCP 응답 타임아웃: {what}"));
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("JSON 파싱: {line}"))
    };
    let mut send = |v: serde_json::Value| {
        stdin
            .write_all(serde_json::to_string(&v).unwrap().as_bytes())
            .unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"mcp_schema","version":"0"}}}),
    );
    let _ = recv("initialize");
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = recv("tools/list");

    drop(stdin);
    let _ = child.wait();

    let tools = list["result"]["tools"].clone();
    assert!(
        tools.is_array(),
        "tools/list 응답에 배열이 없습니다: {list}"
    );
    tools
}

/// `serde_json` is on default features, so object keys are a `BTreeMap` and serialize sorted;
/// array order is `tool_defs()` order. The rendering is therefore stable across runs and platforms.
fn render(tools: &serde_json::Value) -> String {
    let mut text = serde_json::to_string_pretty(tools).expect("직렬화");
    text.push('\n');
    text
}

#[test]
fn mcp_tools_list_schema_matches_the_committed_snapshot() {
    let rendered = render(&tools_list());
    let path = golden_path();

    if std::env::var_os("HWP_UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("golden 디렉터리");
        std::fs::write(&path, &rendered).expect("golden 기록");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} 를 읽을 수 없습니다 ({error}). 최초 생성: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test mcp_schema",
            path.display()
        )
    });
    assert_eq!(
        committed, rendered,
        "MCP 도구 스키마가 커밋된 스냅샷과 다릅니다. \
         의도한 변경이면 갱신하세요: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test mcp_schema"
    );
}
