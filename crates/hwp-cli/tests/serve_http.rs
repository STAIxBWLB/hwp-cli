//! `hwp serve` HTTP adapter surface — the container-deployment contract of
//! docs/design/22-remote-mcp-deployment.md §3.2.
//!
//! Every case drives a real `hwp serve` process over a real socket. The server binds
//! `127.0.0.1:0` and prints the chosen port on its first stderr line, so the tests never
//! guess a port. Documents are produced by the server itself (`hwp_new`), so this suite
//! needs no fixture and no font, which keeps it CI-safe on all three platforms.
//!
//! The HTTP client here is a hand-rolled `TcpStream` exchange rather than a new
//! dev-dependency; a request/response pair over `Connection: close` is a few lines.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The 20 tools the MCP surface publishes; must agree with `cli_surface.rs`.
const EXPECTED_TOOLS: [&str; 20] = [
    "hwp_certify",
    "hwp_compare",
    "hwp_compose",
    "hwp_convert",
    "hwp_diff",
    "hwp_edit",
    "hwp_fill",
    "hwp_grep",
    "hwp_info",
    "hwp_lint",
    "hwp_list_bookmarks",
    "hwp_list_fields",
    "hwp_merge",
    "hwp_new",
    "hwp_read",
    "hwp_render",
    "hwp_slots",
    "hwp_split",
    "hwp_template",
    "hwp_validate",
];

/// A running server; killed on drop so a failing assertion never leaks a process.
struct Serve {
    child: Child,
    addr: String,
    root: PathBuf,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-serve-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn(test: &str, files: bool) -> Serve {
    let root = tmp_dir(test);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hwp"));
    cmd.arg("serve")
        .arg("--addr")
        .arg("127.0.0.1:0")
        .arg("--root")
        .arg(&root);
    if files {
        cmd.arg("--files");
    }
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The bound address arrives on the first stderr line.
    let stderr = child.stderr.take().unwrap();
    let mut first = String::new();
    BufReader::new(stderr).read_line(&mut first).unwrap();
    let addr = first
        .trim()
        .rsplit("http://")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!addr.is_empty(), "바인드 주소를 읽지 못했습니다: {first:?}");

    Serve { child, addr, root }
}

/// Minimal HTTP/1.1 exchange. Returns (status, body).
fn request(addr: &str, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let (status, _, body) = request_full(addr, method, path, body);
    (status, body)
}

/// Same, but also returns the raw header block so tests can assert on `Allow`.
fn request_full(addr: &str, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("헤더와 본문 경계를 찾지 못했습니다");
    let headers = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("상태 줄을 해석하지 못했습니다");
    (status, headers, raw[split + 4..].to_vec())
}

fn rpc(addr: &str, payload: &str) -> (u16, serde_json::Value) {
    let (status, body) = request(addr, "POST", "/mcp", payload.as_bytes());
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON 응답을 해석하지 못했습니다")
    };
    (status, value)
}

fn call_tool(addr: &str, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    });
    let (status, value) = rpc(addr, &payload.to_string());
    assert_eq!(status, 200, "{name} 호출이 200이 아닙니다");
    value["result"].clone()
}

#[test]
fn serve_speaks_the_same_protocol_as_stdio() {
    let server = spawn("session", false);
    let addr = &server.addr;

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200);
    assert_eq!(body, b"ok");

    let (status, value) = rpc(
        addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );
    assert_eq!(status, 200);
    assert_eq!(value["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(value["result"]["serverInfo"]["name"], "hwp-cli");

    // 알림에는 프로토콜 응답이 없다.
    let (status, body) = request(
        addr,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!(status, 202);
    assert!(body.is_empty(), "알림 응답 본문이 비어야 합니다");

    let (status, value) = rpc(addr, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    assert_eq!(status, 200);
    let mut names: Vec<&str> = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, EXPECTED_TOOLS, "도구 20종");
}

#[test]
fn serve_runs_tools_inside_the_root() {
    let server = spawn("tools", false);
    let addr = &server.addr;
    let made = server.root.join("made.hwpx");

    let result = call_tool(
        addr,
        "hwp_new",
        serde_json::json!({"output": made.to_str().unwrap(), "markdown": "# 제목\n\n본문."}),
    );
    assert_eq!(result["isError"], false, "hwp_new: {result}");
    assert!(made.exists(), "문서가 생성되지 않았습니다");

    let result = call_tool(
        addr,
        "hwp_info",
        serde_json::json!({"path": made.to_str().unwrap()}),
    );
    assert_eq!(result["isError"], false, "hwp_info: {result}");

    // --root 밖 쓰기는 stdio와 동일하게 거부된다.
    let outside = std::env::temp_dir().join("hwp-serve-escape.hwpx");
    let result = call_tool(
        addr,
        "hwp_new",
        serde_json::json!({"output": outside.to_str().unwrap(), "markdown": "x"}),
    );
    assert_eq!(result["isError"], true, "root 밖 쓰기가 허용되었습니다");
    assert!(!outside.exists(), "root 밖에 파일이 생겼습니다");
}

#[test]
fn serve_rejects_oversized_and_unsupported_requests() {
    let server = spawn("limits", false);
    let addr = &server.addr;

    // 1 MiB 초과 본문은 파싱 전에 거부한다.
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let (status, _) = request(addr, "POST", "/mcp", &oversized);
    assert_eq!(status, 413);

    // server push가 없으므로 SSE stream을 제공하지 않는다.
    let (status, headers, _) = request_full(addr, "GET", "/mcp", b"");
    assert_eq!(status, 405);
    assert!(
        headers.to_ascii_lowercase().contains("allow: post"),
        "405 응답에 Allow 헤더가 없습니다: {headers}"
    );

    let (status, _) = request(addr, "GET", "/nope", b"");
    assert_eq!(status, 404);

    // --files 없이는 파일 라우트가 존재하지 않는다.
    let (status, _) = request(addr, "POST", "/files/a.bin", b"data");
    assert_eq!(status, 404);
}

#[test]
fn serve_files_roundtrip_and_name_rules() {
    let server = spawn("files", true);
    let addr = &server.addr;

    let (status, _) = request(addr, "POST", "/files/a.bin", b"hello-hwp");
    assert_eq!(status, 200);
    let (status, body) = request(addr, "GET", "/files/a.bin", b"");
    assert_eq!(status, 200);
    assert_eq!(body, b"hello-hwp");

    let (status, _) = request(addr, "GET", "/files/missing.bin", b"");
    assert_eq!(status, 404);

    // 허용 문자에 `/`와 선행 `.`이 없으므로 traversal이 성립하지 않는다.
    for name in [".hidden", "-dash", "%2e%2e", "%ED%95%9C.hwpx"] {
        let (status, _) = request(addr, "POST", &format!("/files/{name}"), b"data");
        assert_eq!(status, 400, "이름 {name} 이 거부되지 않았습니다");
    }

    let entries: Vec<_> = std::fs::read_dir(&server.root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.bin"], "workspace에 예상 밖 파일이 있습니다");
}

#[test]
fn serve_refuses_to_start_without_a_usable_root() {
    let missing_root = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["serve", "--addr", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(
        !missing_root.status.success(),
        "--root 없이 기동이 성공했습니다"
    );

    let absent = std::env::temp_dir().join(format!("hwp-serve-absent-{}", std::process::id()));
    assert!(!Path::new(&absent).exists());
    let bad_root = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["serve", "--addr", "127.0.0.1:0", "--root"])
        .arg(&absent)
        .output()
        .unwrap();
    assert!(
        !bad_root.status.success(),
        "존재하지 않는 --root 로 기동이 성공했습니다"
    );
}
